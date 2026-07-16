import { useCallback, useEffect, useMemo, useRef, useState, type ChangeEvent } from 'react';
import clsx from 'clsx';
import {
    AlertCircle,
    CheckCircle2,
    Clock3,
    Download,
    FolderOpen,
    Loader2,
    LogIn,
    RefreshCw,
    RotateCcw,
    Search,
    ShieldCheck,
    Square,
    Star,
    Trash2,
    Upload,
    UsersRound,
    X,
} from 'lucide-react';
import { appConfirm } from '../utils/appDialogs';
import { resolveRuntimeAssetUrl } from './settings/shared';
import type {
    SocialPublisherPlatform,
    SocialToolsConfig,
    SocialToolsStatus,
} from './settings/SettingsSections';

type SocialConnectionStatus = NonNullable<NonNullable<SocialToolsStatus>['socialConnection']>;
type AccountFile = SocialConnectionStatus['discoveredAccounts'][number];
type AccountRecord = NonNullable<SocialConnectionStatus['accountRecords']>[number];
type ActiveLogin = NonNullable<SocialConnectionStatus['activeLogins']>[number];

type AccountRow = AccountFile & {
    key: string;
    isDefault: boolean;
    record?: AccountRecord;
    activeLogin?: ActiveLogin;
};

type CheckResult = {
    success: boolean;
    status: string;
    message: string;
    checkedAt: number;
};

type LoginState = {
    platform: SocialPublisherPlatform;
    account: string;
    running: boolean;
    success: boolean;
    loggedIn: boolean;
    status: string;
    message: string;
    qrcodeUrl: string;
    qrcodePath: string;
    accountFile: string;
    accountExists: boolean;
    accountUpdatedAt?: number;
    currentUrl?: string;
    startedAt?: number;
    finishedAt?: number;
    lastPolledAt: number;
};

const PLATFORM_OPTIONS: Array<{
    id: SocialPublisherPlatform;
    label: string;
    loginLabel: string;
}> = [
    { id: 'douyin', label: '抖音', loginLabel: '扫码登录' },
    { id: 'xiaohongshu', label: '小红书', loginLabel: '扫码登录' },
    { id: 'kuaishou', label: '快手', loginLabel: '扫码登录' },
    { id: 'bilibili', label: 'Bilibili', loginLabel: '扫码登录' },
    { id: 'tencent', label: '视频号', loginLabel: '扫码登录' },
    { id: 'youtube', label: 'YouTube', loginLabel: '浏览器登录' },
];

const platformLabel = (platform: string) => (
    PLATFORM_OPTIONS.find((item) => item.id === platform)?.label || platform
);

const accountKey = (platform: string, account: string) => `${platform}:${account}`;

function formatTime(value?: number): string {
    if (!value) return '-';
    const date = new Date(value);
    return Number.isNaN(date.getTime()) ? '-' : date.toLocaleString('zh-CN', { hour12: false });
}

function statusLabel(status?: string): string {
    switch (String(status || '').trim()) {
        case 'starting': return '正在启动';
        case 'pending': return '等待页面';
        case 'awaiting_scan': return '等待扫码';
        case 'scanned': return '已扫码待确认';
        case 'verification_required': return '需要安全验证';
        case 'saving': return '正在保存';
        case 'verifying': return '在线校验中';
        case 'logged_in': return '登录有效';
        case 'saved_unverified': return '已保存待复查';
        case 'saved': return '已保存未检查';
        case 'expired': return '已失效';
        case 'failed':
        case 'error': return '失败';
        case 'stopped': return '已停止';
        case 'logging_in': return '登录中';
        default: return status || '未检查';
    }
}

function statusTone(status?: string): string {
    switch (String(status || '').trim()) {
        case 'logged_in':
            return 'bg-emerald-500/10 text-emerald-700 border-emerald-500/20';
        case 'starting':
        case 'pending':
        case 'awaiting_scan':
        case 'scanned':
        case 'verification_required':
        case 'saving':
        case 'verifying':
        case 'logging_in':
            return 'bg-blue-500/10 text-blue-700 border-blue-500/20';
        case 'saved':
        case 'saved_unverified':
            return 'bg-amber-500/10 text-amber-700 border-amber-500/20';
        case 'expired':
        case 'failed':
        case 'error':
            return 'bg-red-500/10 text-red-700 border-red-500/20';
        default:
            return 'bg-surface-secondary text-text-secondary border-border';
    }
}

function normalizeLoginResult(
    result: Record<string, unknown> | null | undefined,
    platform: SocialPublisherPlatform,
    account: string,
): LoginState {
    const status = String(result?.status || 'pending');
    const loggedIn = Boolean(result?.loggedIn) || status === 'logged_in';
    return {
        platform,
        account: String(result?.account || account),
        running: Boolean(result?.running),
        success: Boolean(result?.success),
        loggedIn,
        status,
        message: String(result?.message || result?.error || '登录状态已更新。'),
        qrcodeUrl: String(result?.qrcodeUrl || ''),
        qrcodePath: String(result?.qrcodePath || ''),
        accountFile: String(result?.accountFile || ''),
        accountExists: Boolean(result?.accountExists),
        accountUpdatedAt: typeof result?.accountUpdatedAt === 'number' ? result.accountUpdatedAt : undefined,
        currentUrl: String(result?.currentUrl || ''),
        startedAt: typeof result?.startedAt === 'number' ? result.startedAt : undefined,
        finishedAt: typeof result?.finishedAt === 'number' ? result.finishedAt : undefined,
        lastPolledAt: Date.now(),
    };
}

export function SocialAccounts({ isActive = true }: { isActive?: boolean }) {
    const [status, setStatus] = useState<SocialToolsStatus>(null);
    const [loading, setLoading] = useState(false);
    const [error, setError] = useState('');
    const [message, setMessage] = useState('');
    const [query, setQuery] = useState('');
    const [platformFilter, setPlatformFilter] = useState<'all' | SocialPublisherPlatform>('all');
    const [newPlatform, setNewPlatform] = useState<SocialPublisherPlatform>('douyin');
    const [newAccount, setNewAccount] = useState('');
    const [selectedKeys, setSelectedKeys] = useState<Set<string>>(() => new Set());
    const [operations, setOperations] = useState<Record<string, string | undefined>>({});
    const [checkResults, setCheckResults] = useState<Record<string, CheckResult | undefined>>({});
    const [loginState, setLoginState] = useState<LoginState | null>(null);
    const statusRequestRef = useRef(0);
    const loginPollInFlightRef = useRef(false);
    const importInputRef = useRef<HTMLInputElement | null>(null);

    const loadStatus = useCallback(async () => {
        const requestId = ++statusRequestRef.current;
        setLoading(true);
        try {
            const result = await window.ipcRenderer.socialTools.getStatus() as SocialToolsStatus;
            if (requestId !== statusRequestRef.current) return;
            if (!result?.success) {
                throw new Error(result?.error || '账号池状态读取失败');
            }
            setStatus(result);
            setError('');
            const active = result.socialConnection?.activeLogins?.find((item) => item.running);
            if (active) {
                setLoginState((current) => current || {
                    platform: active.platform as SocialPublisherPlatform,
                    account: active.account,
                    running: active.running,
                    success: active.success,
                    loggedIn: active.status === 'logged_in',
                    status: active.status,
                    message: active.message,
                    qrcodeUrl: '',
                    qrcodePath: '',
                    accountFile: '',
                    accountExists: false,
                    startedAt: active.startedAt,
                    finishedAt: active.finishedAt,
                    lastPolledAt: Date.now(),
                });
            }
        } catch (loadError) {
            if (requestId !== statusRequestRef.current) return;
            setError(String(loadError));
        } finally {
            if (requestId === statusRequestRef.current) setLoading(false);
        }
    }, []);

    useEffect(() => {
        if (!isActive) return;
        void loadStatus();
    }, [isActive, loadStatus]);

    const rows = useMemo<AccountRow[]>(() => {
        const connection = status?.socialConnection;
        if (!connection) return [];
        const byKey = new Map<string, AccountRow>();
        const defaults = status.config.socialConnection.accounts;
        const records = new Map(
            (connection.accountRecords || []).map((record) => [accountKey(record.platform, record.account), record]),
        );
        const activeLogins = new Map(
            (connection.activeLogins || []).map((active) => [accountKey(active.platform, active.account), active]),
        );

        const addFile = (file: AccountFile) => {
            if (!file.exists) return;
            const key = accountKey(file.platform, file.account);
            byKey.set(key, {
                ...file,
                key,
                isDefault: defaults[file.platform as SocialPublisherPlatform] === file.account,
                record: records.get(key),
                activeLogin: activeLogins.get(key),
            });
        };
        for (const file of connection.accounts || []) addFile(file);
        for (const file of connection.discoveredAccounts || []) addFile(file);
        for (const active of connection.activeLogins || []) {
            const key = accountKey(active.platform, active.account);
            if (byKey.has(key)) {
                const current = byKey.get(key)!;
                byKey.set(key, { ...current, activeLogin: active });
                continue;
            }
            if (!active.running) continue;
            byKey.set(key, {
                key,
                platform: active.platform,
                account: active.account,
                cookiePath: '',
                exists: false,
                size: 0,
                updatedAt: undefined,
                validJson: false,
                cookieCount: 0,
                isDefault: defaults[active.platform as SocialPublisherPlatform] === active.account,
                record: records.get(key),
                activeLogin: active,
            });
        }

        return Array.from(byKey.values()).sort((left, right) => {
            if (left.platform !== right.platform) return left.platform.localeCompare(right.platform);
            if (left.isDefault !== right.isDefault) return left.isDefault ? -1 : 1;
            return left.account.localeCompare(right.account);
        });
    }, [status]);

    const visibleRows = useMemo(() => {
        const needle = query.trim().toLocaleLowerCase();
        return rows.filter((row) => {
            if (platformFilter !== 'all' && row.platform !== platformFilter) return false;
            if (!needle) return true;
            return `${platformLabel(row.platform)} ${row.platform} ${row.account}`
                .toLocaleLowerCase()
                .includes(needle);
        });
    }, [platformFilter, query, rows]);

    const activeCount = rows.filter((row) => row.activeLogin?.running).length;
    const validCount = rows.filter((row) => row.record?.status === 'logged_in').length;

    const setOperation = useCallback((key: string, value?: string) => {
        setOperations((current) => ({ ...current, [key]: value }));
    }, []);

    const startLogin = useCallback(async (platform: SocialPublisherPlatform, rawAccount: string) => {
        const account = rawAccount.trim();
        if (!account) {
            setMessage('请先填写团队内部使用的账号 profile 名称，例如 dy_brand_01。');
            return;
        }
        const key = accountKey(platform, account);
        setOperation(key, 'login');
        setMessage('');
        setLoginState({
            platform,
            account,
            running: true,
            success: false,
            loggedIn: false,
            status: 'starting',
            message: `正在为 ${platformLabel(platform)} / ${account} 启动独立浏览器 profile。`,
            qrcodeUrl: '',
            qrcodePath: '',
            accountFile: '',
            accountExists: false,
            lastPolledAt: Date.now(),
        });
        try {
            const config = status?.config;
            const result = await window.ipcRenderer.socialTools.startLogin({
                platform,
                account,
                headless: config?.socialConnection.headless,
                proxyUrl: config?.socialConnection.proxyUrl,
                restart: true,
            });
            const next = normalizeLoginResult(result, platform, account);
            setLoginState(next);
            await loadStatus();
        } catch (loginError) {
            setLoginState((current) => current ? {
                ...current,
                running: false,
                success: false,
                status: 'failed',
                message: String(loginError),
                lastPolledAt: Date.now(),
            } : current);
        } finally {
            setOperation(key);
        }
    }, [loadStatus, setOperation, status?.config]);

    useEffect(() => {
        const platform = loginState?.platform;
        const account = loginState?.account;
        if (!loginState?.running || !platform || !account) return undefined;
        let disposed = false;
        const poll = async () => {
            if (disposed || loginPollInFlightRef.current) return;
            loginPollInFlightRef.current = true;
            try {
                const result = await window.ipcRenderer.socialTools.getLoginStatus({ platform, account });
                if (disposed) return;
                const next = normalizeLoginResult(result, platform, account);
                setLoginState(next);
                if (!next.running) await loadStatus();
            } catch (pollError) {
                if (!disposed) {
                    setLoginState((current) => current ? {
                        ...current,
                        message: `状态轮询失败：${String(pollError)}`,
                        lastPolledAt: Date.now(),
                    } : current);
                }
            } finally {
                loginPollInFlightRef.current = false;
            }
        };
        const timer = window.setInterval(() => void poll(), 1_000);
        return () => {
            disposed = true;
            window.clearInterval(timer);
        };
    }, [loadStatus, loginState?.account, loginState?.platform, loginState?.running]);

    const stopLogin = useCallback(async () => {
        if (!loginState) return;
        const { platform, account } = loginState;
        const result = await window.ipcRenderer.socialTools.stopLogin({ platform, account });
        setLoginState(normalizeLoginResult(result, platform, account));
        await loadStatus();
    }, [loadStatus, loginState]);

    const openOrRestartLogin = useCallback(async (row: AccountRow) => {
        const platform = row.platform as SocialPublisherPlatform;
        if (!row.activeLogin?.running) {
            await startLogin(platform, row.account);
            return;
        }
        const result = await window.ipcRenderer.socialTools.getLoginStatus({
            platform,
            account: row.account,
        });
        setLoginState(normalizeLoginResult(result, platform, row.account));
    }, [startLogin]);

    const checkAccount = useCallback(async (row: AccountRow, refreshAfter = true) => {
        const key = row.key;
        setOperation(key, 'check');
        try {
            const result = await window.ipcRenderer.socialTools.checkAccount({
                platform: row.platform,
                account: row.account,
                proxyUrl: status?.config.socialConnection.proxyUrl,
            });
            setCheckResults((current) => ({
                ...current,
                [key]: {
                    success: Boolean(result?.success),
                    status: String(result?.status || (result?.success ? 'logged_in' : 'expired')),
                    message: String(result?.message || result?.error || '检查完成。'),
                    checkedAt: Date.now(),
                },
            }));
            if (refreshAfter) await loadStatus();
        } catch (checkError) {
            setCheckResults((current) => ({
                ...current,
                [key]: {
                    success: false,
                    status: 'error',
                    message: String(checkError),
                    checkedAt: Date.now(),
                },
            }));
        } finally {
            setOperation(key);
        }
    }, [loadStatus, setOperation, status?.config.socialConnection.proxyUrl]);

    const checkSelected = useCallback(async () => {
        const targets = rows.filter((row) => selectedKeys.has(row.key));
        if (targets.length === 0) {
            setMessage('请先勾选需要检查的账号。');
            return;
        }
        setMessage(`正在检查 ${targets.length} 个账号；同时最多启动 2 个浏览器，避免占用过多资源。`);
        let nextIndex = 0;
        const worker = async () => {
            while (nextIndex < targets.length) {
                const row = targets[nextIndex++];
                await checkAccount(row, false);
            }
        };
        await Promise.all(Array.from({ length: Math.min(2, targets.length) }, () => worker()));
        await loadStatus();
        setMessage(`${targets.length} 个账号检查完成。`);
    }, [checkAccount, loadStatus, rows, selectedKeys]);

    const setDefaultAccount = useCallback(async (row: AccountRow) => {
        const config = status?.config;
        if (!config) return;
        const platform = row.platform as SocialPublisherPlatform;
        const nextConfig: SocialToolsConfig = {
            ...config,
            socialConnection: {
                ...config.socialConnection,
                accounts: {
                    ...config.socialConnection.accounts,
                    [platform]: row.account,
                },
            },
        };
        setOperation(row.key, 'default');
        try {
            const result = await window.ipcRenderer.socialTools.saveConfig(nextConfig);
            if (!result?.success) throw new Error(result?.error || '默认账号保存失败');
            setMessage(`${platformLabel(platform)} 默认发布账号已切换为 ${row.account}。`);
            await loadStatus();
        } finally {
            setOperation(row.key);
        }
    }, [loadStatus, setOperation, status?.config]);

    const exportAccount = useCallback(async (row: AccountRow) => {
        setOperation(row.key, 'export');
        try {
            const result = await window.ipcRenderer.socialTools.exportAccount({
                platform: row.platform,
                account: row.account,
            });
            if (!result?.success || typeof result.content !== 'string') {
                throw new Error(result?.error || result?.message || '导出失败');
            }
            const blob = new Blob([result.content], { type: 'application/json;charset=utf-8' });
            const url = URL.createObjectURL(blob);
            const anchor = document.createElement('a');
            anchor.href = url;
            anchor.download = String(result.filename || `${row.platform}_${row.account}.json`);
            document.body.appendChild(anchor);
            anchor.click();
            anchor.remove();
            URL.revokeObjectURL(url);
        } finally {
            setOperation(row.key);
        }
    }, [setOperation]);

    const deleteAccount = useCallback(async (row: AccountRow) => {
        const confirmed = await appConfirm(
            `确认删除 ${platformLabel(row.platform)} / ${row.account}？账号文件和独立浏览器 profile 都会删除。`,
            { title: '删除社媒账号', confirmLabel: '删除', tone: 'danger' },
        );
        if (!confirmed) return;
        setOperation(row.key, 'delete');
        try {
            const result = await window.ipcRenderer.socialTools.deleteAccount({
                platform: row.platform,
                account: row.account,
            });
            if (!result?.success) throw new Error(result?.error || result?.message || '删除失败');
            if (row.isDefault && status?.config) {
                const platform = row.platform as SocialPublisherPlatform;
                const replacement = rows.find((item) => item.platform === row.platform && item.key !== row.key)?.account || 'default';
                const nextConfig: SocialToolsConfig = {
                    ...status.config,
                    socialConnection: {
                        ...status.config.socialConnection,
                        accounts: {
                            ...status.config.socialConnection.accounts,
                            [platform]: replacement,
                        },
                    },
                };
                await window.ipcRenderer.socialTools.saveConfig(nextConfig);
            }
            setSelectedKeys((current) => {
                const next = new Set(current);
                next.delete(row.key);
                return next;
            });
            setMessage(`${platformLabel(row.platform)} / ${row.account} 已删除。`);
            await loadStatus();
        } finally {
            setOperation(row.key);
        }
    }, [loadStatus, rows, setOperation, status?.config]);

    const importAccount = useCallback(async (event: ChangeEvent<HTMLInputElement>) => {
        const file = event.target.files?.[0];
        event.target.value = '';
        if (!file) return;
        const account = newAccount.trim();
        if (!account) {
            setMessage('导入前请填写账号 profile 名称。');
            return;
        }
        const key = accountKey(newPlatform, account);
        setOperation(key, 'import');
        try {
            const content = await file.text();
            const result = await window.ipcRenderer.socialTools.importAccount({
                platform: newPlatform,
                account,
                content,
            });
            if (!result?.success) throw new Error(result?.error || result?.message || '导入失败');
            setMessage(`${platformLabel(newPlatform)} / ${account} 已导入；建议立即执行在线检查。`);
            await loadStatus();
        } finally {
            setOperation(key);
        }
    }, [loadStatus, newAccount, newPlatform, setOperation]);

    const toggleSelected = (key: string) => {
        setSelectedKeys((current) => {
            const next = new Set(current);
            if (next.has(key)) next.delete(key);
            else next.add(key);
            return next;
        });
    };

    const allVisibleSelected = visibleRows.length > 0 && visibleRows.every((row) => selectedKeys.has(row.key));
    const toggleAllVisible = () => {
        setSelectedKeys((current) => {
            const next = new Set(current);
            for (const row of visibleRows) {
                if (allVisibleSelected) next.delete(row.key);
                else next.add(row.key);
            }
            return next;
        });
    };

    return (
        <div className="flex h-full min-h-0 flex-col bg-background">
            <header className="shrink-0 border-b border-border bg-surface-primary/70 px-6 py-4">
                <div className="flex flex-wrap items-start justify-between gap-4">
                    <div>
                        <div className="flex items-center gap-2">
                            <UsersRound className="h-5 w-5 text-accent-primary" />
                            <h1 className="text-lg font-semibold text-text-primary">Social Connection 账号池</h1>
                        </div>
                        <p className="mt-1 text-xs text-text-tertiary">
                            每个平台可保存任意数量的团队账号；每个 profile 都使用独立 Cookie 文件和浏览器目录。
                        </p>
                    </div>
                    <div className="flex items-center gap-2">
                        <button
                            type="button"
                            onClick={() => void window.ipcRenderer.socialTools.openSocialCookiesDir()}
                            className="inline-flex items-center gap-1.5 rounded-lg border border-border px-3 py-2 text-xs text-text-secondary hover:bg-surface-secondary"
                        >
                            <FolderOpen className="h-3.5 w-3.5" />
                            打开账号目录
                        </button>
                        <button
                            type="button"
                            onClick={() => void loadStatus()}
                            disabled={loading}
                            className="inline-flex items-center gap-1.5 rounded-lg border border-border px-3 py-2 text-xs text-text-secondary hover:bg-surface-secondary disabled:opacity-50"
                        >
                            <RefreshCw className={clsx('h-3.5 w-3.5', loading && 'animate-spin')} />
                            刷新
                        </button>
                    </div>
                </div>
            </header>

            <main className="min-h-0 flex-1 overflow-auto px-6 py-5">
                <div className="mx-auto max-w-[1500px] space-y-4">
                    {error ? (
                        <div className="flex items-start gap-2 rounded-lg border border-red-500/20 bg-red-500/10 px-3 py-2 text-xs text-red-700">
                            <AlertCircle className="mt-0.5 h-4 w-4 shrink-0" />
                            {error}
                        </div>
                    ) : null}
                    {message ? (
                        <div className="rounded-lg border border-border bg-surface-primary px-3 py-2 text-xs text-text-secondary">
                            {message}
                        </div>
                    ) : null}

                    <div className="grid gap-3 md:grid-cols-4">
                        <div className="rounded-xl border border-border bg-surface-primary p-4">
                            <div className="text-[11px] text-text-tertiary">账号总数</div>
                            <div className="mt-1 text-2xl font-semibold text-text-primary">{rows.length}</div>
                            <div className="mt-1 text-[11px] text-text-tertiary">跨 6 个发布平台</div>
                        </div>
                        <div className="rounded-xl border border-border bg-surface-primary p-4">
                            <div className="text-[11px] text-text-tertiary">已在线验证</div>
                            <div className="mt-1 text-2xl font-semibold text-emerald-600">{validCount}</div>
                            <div className="mt-1 text-[11px] text-text-tertiary">最近检查状态为有效</div>
                        </div>
                        <div className="rounded-xl border border-border bg-surface-primary p-4">
                            <div className="text-[11px] text-text-tertiary">登录任务</div>
                            <div className="mt-1 text-2xl font-semibold text-blue-600">{activeCount}</div>
                            <div className="mt-1 text-[11px] text-text-tertiary">支持不同账号同时登录</div>
                        </div>
                        <div className="rounded-xl border border-border bg-surface-primary p-4">
                            <div className="text-[11px] text-text-tertiary">Rust CDP Runtime</div>
                            <div className={clsx(
                                'mt-1 text-sm font-semibold',
                                status?.socialConnection?.browserAvailable ? 'text-emerald-600' : 'text-red-600',
                            )}>
                                {status?.socialConnection?.browserAvailable ? '浏览器可用' : '未找到浏览器'}
                            </div>
                            <div className="mt-1 truncate text-[11px] text-text-tertiary">
                                {status?.socialConnection?.browserExecutable || '自动探测 Chrome / Edge / Chromium'}
                            </div>
                        </div>
                    </div>

                    <section className="rounded-xl border border-border bg-surface-primary p-4">
                        <div className="flex flex-wrap items-end gap-3">
                            <div className="min-w-[150px]">
                                <label className="mb-1 block text-[11px] text-text-tertiary">平台</label>
                                <select
                                    value={newPlatform}
                                    aria-label="新账号平台"
                                    onChange={(event) => setNewPlatform(event.target.value as SocialPublisherPlatform)}
                                    className="w-full rounded-lg border border-border bg-background px-3 py-2 text-sm text-text-primary"
                                >
                                    {PLATFORM_OPTIONS.map((item) => (
                                        <option key={item.id} value={item.id}>{item.label}</option>
                                    ))}
                                </select>
                            </div>
                            <div className="min-w-[260px] flex-1">
                                <label className="mb-1 block text-[11px] text-text-tertiary">新账号 profile</label>
                                <input
                                    value={newAccount}
                                    onChange={(event) => setNewAccount(event.target.value)}
                                    onKeyDown={(event) => {
                                        if (event.key === 'Enter') void startLogin(newPlatform, newAccount);
                                    }}
                                    placeholder="例如：dy_brand_01、xhs_editor_02"
                                    aria-label="新账号 profile"
                                    className="w-full rounded-lg border border-border bg-background px-3 py-2 text-sm text-text-primary outline-none focus:border-accent-primary"
                                />
                            </div>
                            <button
                                type="button"
                                onClick={() => void startLogin(newPlatform, newAccount)}
                                disabled={!newAccount.trim() || operations[accountKey(newPlatform, newAccount.trim())] === 'login'}
                                className="inline-flex items-center gap-1.5 rounded-lg bg-accent-primary px-4 py-2 text-sm text-white hover:opacity-90 disabled:opacity-50"
                            >
                                <LogIn className="h-4 w-4" />
                                {PLATFORM_OPTIONS.find((item) => item.id === newPlatform)?.loginLabel}
                            </button>
                            <input
                                ref={importInputRef}
                                type="file"
                                accept=".json,application/json"
                                className="hidden"
                                onChange={(event) => void importAccount(event)}
                            />
                            <button
                                type="button"
                                onClick={() => importInputRef.current?.click()}
                                disabled={!newAccount.trim()}
                                className="inline-flex items-center gap-1.5 rounded-lg border border-border px-4 py-2 text-sm text-text-secondary hover:bg-surface-secondary disabled:opacity-50"
                            >
                                <Upload className="h-4 w-4" />
                                导入账号文件
                            </button>
                        </div>
                        <p className="mt-2 text-[11px] text-text-tertiary">
                            profile 是团队内部名称，不是平台密码。相同平台可重复新增不同 profile，并分别扫码保存。
                        </p>
                    </section>

                    <section className="overflow-hidden rounded-xl border border-border bg-surface-primary">
                        <div className="flex flex-wrap items-center justify-between gap-3 border-b border-border px-4 py-3">
                            <div className="flex flex-wrap items-center gap-2">
                                <div className="relative min-w-[260px]">
                                    <Search className="pointer-events-none absolute left-3 top-1/2 h-3.5 w-3.5 -translate-y-1/2 text-text-tertiary" />
                                    <input
                                        value={query}
                                        onChange={(event) => setQuery(event.target.value)}
                                        placeholder="搜索平台或 profile"
                                        aria-label="搜索社媒平台或 profile"
                                        className="w-full rounded-lg border border-border bg-background py-2 pl-9 pr-3 text-xs text-text-primary outline-none focus:border-accent-primary"
                                    />
                                </div>
                                <select
                                    value={platformFilter}
                                    aria-label="社媒平台筛选"
                                    onChange={(event) => setPlatformFilter(event.target.value as 'all' | SocialPublisherPlatform)}
                                    className="rounded-lg border border-border bg-background px-3 py-2 text-xs text-text-primary"
                                >
                                    <option value="all">全部平台</option>
                                    {PLATFORM_OPTIONS.map((item) => (
                                        <option key={item.id} value={item.id}>{item.label}</option>
                                    ))}
                                </select>
                            </div>
                            <button
                                type="button"
                                onClick={() => void checkSelected()}
                                disabled={selectedKeys.size === 0}
                                className="inline-flex items-center gap-1.5 rounded-lg border border-border px-3 py-2 text-xs text-text-secondary hover:bg-surface-secondary disabled:opacity-50"
                            >
                                <ShieldCheck className="h-3.5 w-3.5" />
                                检查所选 ({selectedKeys.size})
                            </button>
                        </div>

                        {loading && rows.length === 0 ? (
                            <div className="flex items-center justify-center py-16 text-sm text-text-tertiary">
                                <Loader2 className="mr-2 h-4 w-4 animate-spin" />
                                正在读取账号池...
                            </div>
                        ) : visibleRows.length === 0 ? (
                            <div className="py-16 text-center text-sm text-text-tertiary">
                                暂未发现符合条件的账号。可在上方输入 profile 后扫码登录。
                            </div>
                        ) : (
                            <div className="overflow-auto">
                                <table className="w-full min-w-[1180px] text-left text-xs">
                                    <thead className="bg-surface-secondary/70 text-[11px] text-text-tertiary">
                                        <tr>
                                            <th className="w-10 px-3 py-2.5">
                                                <button type="button" onClick={toggleAllVisible} aria-label="选择当前筛选账号">
                                                    {allVisibleSelected
                                                        ? <CheckCircle2 className="h-4 w-4 text-accent-primary" />
                                                        : <Square className="h-4 w-4" />}
                                                </button>
                                            </th>
                                            <th className="px-3 py-2.5 font-medium">平台 / profile</th>
                                            <th className="px-3 py-2.5 font-medium">状态</th>
                                            <th className="px-3 py-2.5 font-medium">账号文件</th>
                                            <th className="px-3 py-2.5 font-medium">最近更新</th>
                                            <th className="px-3 py-2.5 font-medium">操作</th>
                                        </tr>
                                    </thead>
                                    <tbody className="divide-y divide-border">
                                        {visibleRows.map((row) => {
                                            const operation = operations[row.key];
                                            const check = checkResults[row.key];
                                            const effectiveStatus = row.activeLogin?.status || check?.status || row.record?.status || (row.exists ? 'saved' : 'idle');
                                            const statusMessage = row.activeLogin?.message || check?.message || row.record?.errorMessage || '';
                                            return (
                                                <tr key={row.key} className="hover:bg-surface-secondary/35">
                                                    <td className="px-3 py-3 align-top">
                                                        <button type="button" onClick={() => toggleSelected(row.key)} aria-label={`选择 ${row.account}`}>
                                                            {selectedKeys.has(row.key)
                                                                ? <CheckCircle2 className="h-4 w-4 text-accent-primary" />
                                                                : <Square className="h-4 w-4 text-text-tertiary" />}
                                                        </button>
                                                    </td>
                                                    <td className="px-3 py-3 align-top">
                                                        <div className="flex items-center gap-2 font-medium text-text-primary">
                                                            {platformLabel(row.platform)}
                                                            {row.isDefault ? (
                                                                <span className="inline-flex items-center gap-1 rounded bg-amber-500/10 px-1.5 py-0.5 text-[10px] text-amber-700">
                                                                    <Star className="h-3 w-3 fill-current" /> 默认
                                                                </span>
                                                            ) : null}
                                                        </div>
                                                        <div className="mt-1 font-mono text-[11px] text-text-secondary">{row.account}</div>
                                                    </td>
                                                    <td className="px-3 py-3 align-top">
                                                        <span className={clsx('inline-flex rounded-full border px-2 py-1 text-[10px]', statusTone(effectiveStatus))}>
                                                            {statusLabel(effectiveStatus)}
                                                        </span>
                                                        {statusMessage ? (
                                                            <div className="mt-1 max-w-[240px] text-[10px] leading-4 text-text-tertiary">{statusMessage}</div>
                                                        ) : null}
                                                    </td>
                                                    <td className="px-3 py-3 align-top">
                                                        <div className={row.exists && row.validJson ? 'text-emerald-600' : 'text-amber-600'}>
                                                            {row.exists && row.validJson ? '已保存' : '尚未保存'}
                                                        </div>
                                                        <div className="mt-1 text-[10px] text-text-tertiary">
                                                            Cookie {row.cookieCount || 0} 条 · {row.size || 0} bytes
                                                        </div>
                                                        {row.cookiePath ? (
                                                            <div className="mt-1 max-w-[320px] truncate font-mono text-[10px] text-text-tertiary" title={row.cookiePath}>
                                                                {row.cookiePath}
                                                            </div>
                                                        ) : null}
                                                    </td>
                                                    <td className="px-3 py-3 align-top text-[11px] text-text-secondary">
                                                        <div>{formatTime(row.updatedAt || row.record?.updatedAt)}</div>
                                                        {check ? (
                                                            <div className="mt-1 text-[10px] text-text-tertiary">本页检查 {formatTime(check.checkedAt)}</div>
                                                        ) : null}
                                                    </td>
                                                    <td className="px-3 py-3 align-top">
                                                        <div className="flex flex-wrap gap-1.5">
                                                            <button
                                                                type="button"
                                                                onClick={() => void checkAccount(row)}
                                                                disabled={Boolean(operation)}
                                                                className="rounded border border-border px-2 py-1 text-[11px] text-text-secondary hover:bg-surface-secondary disabled:opacity-50"
                                                            >
                                                                {operation === 'check' ? '检查中' : '检查'}
                                                            </button>
                                                            <button
                                                                type="button"
                                                                onClick={() => void openOrRestartLogin(row)}
                                                                disabled={Boolean(operation)}
                                                                className="rounded border border-border px-2 py-1 text-[11px] text-text-secondary hover:bg-surface-secondary disabled:opacity-50"
                                                            >
                                                                {row.activeLogin?.running ? '查看登录' : '重新登录'}
                                                            </button>
                                                            {!row.isDefault ? (
                                                                <button
                                                                    type="button"
                                                                    onClick={() => void setDefaultAccount(row)}
                                                                    disabled={Boolean(operation)}
                                                                    className="rounded border border-border px-2 py-1 text-[11px] text-text-secondary hover:bg-surface-secondary disabled:opacity-50"
                                                                >
                                                                    设为默认
                                                                </button>
                                                            ) : null}
                                                            <button
                                                                type="button"
                                                                onClick={() => void exportAccount(row)}
                                                                disabled={Boolean(operation) || !row.exists}
                                                                className="inline-flex items-center gap-1 rounded border border-border px-2 py-1 text-[11px] text-text-secondary hover:bg-surface-secondary disabled:opacity-50"
                                                            >
                                                                <Download className="h-3 w-3" /> 导出
                                                            </button>
                                                            <button
                                                                type="button"
                                                                onClick={() => void deleteAccount(row)}
                                                                disabled={Boolean(operation)}
                                                                className="inline-flex items-center gap-1 rounded border border-red-500/20 px-2 py-1 text-[11px] text-red-600 hover:bg-red-500/10 disabled:opacity-50"
                                                            >
                                                                <Trash2 className="h-3 w-3" /> 删除
                                                            </button>
                                                        </div>
                                                    </td>
                                                </tr>
                                            );
                                        })}
                                    </tbody>
                                </table>
                            </div>
                        )}
                    </section>
                </div>
            </main>

            {loginState ? (
                <div className="fixed inset-0 z-[120] flex items-center justify-center bg-black/45 p-5 backdrop-blur-sm">
                    <div className="w-full max-w-2xl overflow-hidden rounded-2xl border border-border bg-background shadow-2xl">
                        <div className="flex items-start justify-between gap-3 border-b border-border px-5 py-4">
                            <div>
                                <div className="flex items-center gap-2">
                                    <h2 className="font-semibold text-text-primary">
                                        {platformLabel(loginState.platform)} / {loginState.account}
                                    </h2>
                                    <span className={clsx('rounded-full border px-2 py-0.5 text-[10px]', statusTone(loginState.status))}>
                                        {statusLabel(loginState.status)}
                                    </span>
                                </div>
                                <p className="mt-1 text-xs text-text-tertiary">每 1 秒自动更新，无需手动点击检查。</p>
                            </div>
                            {!loginState.running ? (
                                <button
                                    type="button"
                                    onClick={() => setLoginState(null)}
                                    className="rounded-lg p-1.5 text-text-tertiary hover:bg-surface-secondary hover:text-text-primary"
                                    aria-label="关闭登录状态"
                                >
                                    <X className="h-4 w-4" />
                                </button>
                            ) : null}
                        </div>

                        <div className="grid gap-5 p-5 md:grid-cols-[240px_1fr]">
                            <div className="flex min-h-[240px] items-center justify-center rounded-xl border border-border bg-white p-3">
                                {loginState.qrcodeUrl ? (
                                    <img
                                        src={resolveRuntimeAssetUrl(loginState.qrcodeUrl)}
                                        alt={`${platformLabel(loginState.platform)} 登录二维码`}
                                        className="max-h-[215px] max-w-full object-contain"
                                    />
                                ) : loginState.platform === 'youtube' ? (
                                    <div className="text-center text-xs text-slate-500">
                                        <LogIn className="mx-auto mb-2 h-8 w-8" />
                                        请在弹出的浏览器中完成 Google 登录
                                    </div>
                                ) : loginState.loggedIn ? (
                                    <div className="text-center text-emerald-600">
                                        <CheckCircle2 className="mx-auto mb-2 h-10 w-10" />
                                        <div className="text-sm font-medium">登录并保存成功</div>
                                    </div>
                                ) : (
                                    <div className="text-center text-xs text-slate-500">
                                        <Loader2 className="mx-auto mb-2 h-7 w-7 animate-spin" />
                                        正在获取二维码或等待页面跳转
                                    </div>
                                )}
                            </div>

                            <div className="space-y-3">
                                <div className={clsx(
                                    'rounded-xl border px-3 py-3 text-sm leading-6',
                                    loginState.loggedIn
                                        ? 'border-emerald-500/20 bg-emerald-500/10 text-emerald-700'
                                        : loginState.status === 'failed' || loginState.status === 'error'
                                            ? 'border-red-500/20 bg-red-500/10 text-red-700'
                                            : 'border-border bg-surface-primary text-text-secondary',
                                )}>
                                    {loginState.message}
                                </div>
                                <div className="space-y-1 rounded-xl border border-border bg-surface-primary px-3 py-3 text-[11px] text-text-tertiary">
                                    <div className="flex items-center gap-2">
                                        <Clock3 className="h-3.5 w-3.5" />
                                        上次自动更新：{formatTime(loginState.lastPolledAt)}
                                    </div>
                                    <div>账号文件：{loginState.accountExists ? '已生成' : '尚未生成'}</div>
                                    {loginState.accountFile ? (
                                        <div className="break-all font-mono">{loginState.accountFile}</div>
                                    ) : null}
                                    {loginState.currentUrl ? (
                                        <div className="break-all font-mono">页面：{loginState.currentUrl}</div>
                                    ) : null}
                                </div>
                                {loginState.status === 'scanned' || loginState.status === 'verification_required' ? (
                                    <div className="rounded-xl border border-blue-500/20 bg-blue-500/10 px-3 py-2 text-xs text-blue-700">
                                        抖音扫码后通常还需要在手机端确认；若出现短信或安全验证，请在弹出的浏览器完成。
                                    </div>
                                ) : null}
                            </div>
                        </div>

                        <div className="flex flex-wrap items-center justify-end gap-2 border-t border-border px-5 py-4">
                            {loginState.running ? (
                                <button
                                    type="button"
                                    onClick={() => void stopLogin()}
                                    className="rounded-lg border border-border px-3 py-2 text-xs text-text-secondary hover:bg-surface-secondary"
                                >
                                    停止登录
                                </button>
                            ) : null}
                            <button
                                type="button"
                                onClick={() => void startLogin(loginState.platform, loginState.account)}
                                className="inline-flex items-center gap-1.5 rounded-lg border border-border px-3 py-2 text-xs text-text-secondary hover:bg-surface-secondary"
                            >
                                <RotateCcw className="h-3.5 w-3.5" />
                                {loginState.running ? '重新生成二维码' : '重新登录'}
                            </button>
                            {!loginState.running ? (
                                <button
                                    type="button"
                                    onClick={() => setLoginState(null)}
                                    className="rounded-lg bg-accent-primary px-3 py-2 text-xs text-white hover:opacity-90"
                                >
                                    完成
                                </button>
                            ) : null}
                        </div>
                    </div>
                </div>
            ) : null}
        </div>
    );
}
