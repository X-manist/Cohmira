import { useEffect, useMemo, useState } from 'react';
import { AlertCircle, ArrowLeft, ArrowRight, Loader2, Sparkles, X } from 'lucide-react';
import { clsx } from 'clsx';
import { Slider } from '../../vendor/freecut/components/ui/slider';
import {
  REDCLAW_ONBOARDING_DEFAULT_ANSWERS,
  REDCLAW_ONBOARDING_MVP_QUESTIONS,
  normalizeOnboardingAnswers,
  onboardingProgressLabel,
  type RedClawOnboardingAnswers,
  type RedClawOnboardingQuestion,
} from './onboardingMvp';

interface RedClawOnboardingFlowProps {
  open: boolean;
  activeSpaceName: string;
  initialStepIndex?: number;
  initialAnswers?: Record<string, unknown> | null;
  onClose: () => void;
  onSaveProgress: (payload: { stepIndex: number; answers: RedClawOnboardingAnswers }) => Promise<void>;
  onComplete: (answers: RedClawOnboardingAnswers) => Promise<void>;
}

const COMPLETION_STAGES = [
  '正在保存问卷结果',
  '正在保存商媒运营助手初始化配置',
  '正在更新空间长期档案',
  '正在更新空间写作风格技能',
  '正在刷新当前空间上下文',
] as const;

function QuestionProgress({
  currentStepIndex,
  submitting = false,
}: {
  currentStepIndex: number;
  submitting?: boolean;
}) {
  const progress = submitting
    ? 100
    : ((currentStepIndex + 1) / REDCLAW_ONBOARDING_MVP_QUESTIONS.length) * 100;
  return (
    <div className="space-y-2">
      <div className="flex items-center justify-between text-[11px] font-semibold uppercase tracking-[0.24em] text-text-secondary">
        <span>Space Initialization</span>
        <span>{submitting ? 'Finalizing' : onboardingProgressLabel(currentStepIndex)}</span>
      </div>
      <div className="h-2 overflow-hidden rounded-full bg-surface-tertiary/60">
        <div
          className="h-full rounded-full bg-gradient-to-r from-accent-primary via-brand-red to-status-success transition-[width] duration-300 ease-out"
          style={{ width: `${progress}%` }}
        />
      </div>
    </div>
  );
}

function CompletionView({
  activeSpaceName,
  stageIndex,
}: {
  activeSpaceName: string;
  stageIndex: number;
}) {
  return (
    <div className="flex h-full min-h-[420px] items-center justify-center">
      <div className="w-full max-w-3xl rounded-[32px] border border-border bg-surface-elevated/95 px-8 py-10 text-center shadow-[var(--ui-shadow-2)] backdrop-blur-xl">
        <div className="mx-auto flex h-16 w-16 items-center justify-center rounded-full border border-accent-primary/40 bg-accent-muted">
          <Loader2 className="h-7 w-7 animate-spin text-accent-primary" />
        </div>
        <div className="mt-6 text-[11px] font-semibold uppercase tracking-[0.24em] text-text-secondary">
          商媒运营助手 · {activeSpaceName || '当前空间'}
        </div>
        <h2 className="mt-3 text-3xl font-semibold leading-tight text-text-primary sm:text-[38px]">
          正在完成这个空间的风格初始化
        </h2>
        <p className="mx-auto mt-4 max-w-2xl text-sm leading-7 text-text-secondary sm:text-[15px]">
          我正在把这份问卷结果写入当前空间档案和写作风格配置。这个页面会一直停留到初始化全部完成。
        </p>

        <div className="mt-8 space-y-3 text-left">
          {COMPLETION_STAGES.map((label, index) => {
            const active = index <= stageIndex;
            return (
              <div
                key={label}
                className={clsx(
                  'flex items-center gap-3 rounded-2xl border px-4 py-3 transition-all duration-200',
                  active
                    ? 'border-accent-primary/40 bg-accent-muted/70 text-text-primary'
                    : 'border-border bg-surface-secondary/70 text-text-tertiary'
                )}
              >
                <div
                  className={clsx(
                    'flex h-6 w-6 items-center justify-center rounded-full border',
                    active ? 'border-accent-primary/50 bg-accent-muted' : 'border-border bg-surface-tertiary/60'
                  )}
                >
                  {index === stageIndex ? <Loader2 className="h-3.5 w-3.5 animate-spin text-accent-primary" /> : <span className="text-[11px] font-semibold">{index + 1}</span>}
                </div>
                <div className="text-sm font-medium">{label}</div>
              </div>
            );
          })}
        </div>
      </div>
    </div>
  );
}

function SliderQuestionView({
  question,
  value,
  onChange,
}: {
  question: Extract<RedClawOnboardingQuestion, { type: 'slider' }>;
  value: number;
  onChange: (next: number) => void;
}) {
  const leftValue = Math.max(0, 100 - value);
  const rightValue = Math.max(0, value);

  return (
    <div className="space-y-8">
      <div className="space-y-4">
        <div className="inline-flex items-center gap-2 rounded-full border border-accent-primary/40 bg-surface-primary/80 px-3 py-1 text-[11px] font-semibold uppercase tracking-[0.18em] text-text-secondary">
          <Sparkles className="h-3.5 w-3.5 text-accent-primary" />
          Continuous Scale
        </div>
        <div className="space-y-3">
          <h2 className="max-w-3xl text-3xl font-semibold leading-tight text-text-primary sm:text-[38px]">
            {question.title}
          </h2>
          <p className="max-w-2xl text-sm leading-7 text-text-secondary sm:text-[15px]">
            {question.description}
          </p>
        </div>
      </div>

      <div className="rounded-[28px] border border-border bg-surface-elevated/90 p-6 shadow-[var(--ui-shadow-2)] backdrop-blur-xl sm:p-8">
        <div className="mb-2 flex items-end justify-between gap-6">
          <div className="min-w-0">
            <div className="text-sm font-semibold text-text-secondary">{question.minLabel}</div>
            <div className="mt-1 text-[34px] font-black leading-none tracking-[-0.03em] text-text-primary">{leftValue}%</div>
          </div>
          <div className="min-w-0 text-right">
            <div className="text-sm font-semibold text-text-secondary">{question.maxLabel}</div>
            <div className="mt-1 text-[34px] font-black leading-none tracking-[-0.03em] text-text-primary">{rightValue}%</div>
          </div>
        </div>
        <div className="mt-1">
          <div className="relative px-16 py-6">
            <div className="absolute inset-x-16 top-1/2 h-12 -translate-y-1/2 overflow-hidden rounded-full bg-accent-muted shadow-inner">
              <div
                className="h-full rounded-full bg-gradient-to-r from-accent-primary via-brand-red to-status-success"
                style={{ width: `${value}%` }}
              />
            </div>

            <Slider
              value={[value]}
              min={0}
              max={100}
              step={1}
              onValueChange={(values) => onChange(values[0] ?? value)}
              className="relative z-20 w-full cursor-grab py-6 active:cursor-grabbing [&>span:first-child]:h-12 [&>span:first-child]:bg-transparent [&>span:first-child>span]:bg-transparent [&>span:last-child]:h-16 [&>span:last-child]:w-16 [&>span:last-child]:rounded-none [&>span:last-child]:border-0 [&>span:last-child]:bg-[url('/cohmira-mark.svg')] [&>span:last-child]:bg-contain [&>span:last-child]:bg-center [&>span:last-child]:bg-no-repeat [&>span:last-child]:bg-transparent [&>span:last-child]:shadow-none [&>span:last-child]:outline-none [&>span:last-child]:ring-0 [&>span:last-child]:focus:outline-none [&>span:last-child]:focus-visible:outline-none [&>span:last-child]:focus-visible:ring-0"
            />
          </div>

          <div className="mt-6 rounded-2xl border border-accent-primary/30 bg-accent-muted/70 px-4 py-3 text-sm leading-6 text-text-secondary">
            {question.helper(value)}
          </div>
        </div>
      </div>
    </div>
  );
}

function ChoiceQuestionView({
  question,
  value,
  onChange,
}: {
  question: Extract<RedClawOnboardingQuestion, { type: 'choice' }>;
  value: string;
  onChange: (next: string) => void;
}) {
  return (
    <div className="space-y-8">
      <div className="space-y-3">
        <h2 className="max-w-3xl text-3xl font-semibold leading-tight text-text-primary sm:text-[38px]">
          {question.title}
        </h2>
        <p className="max-w-2xl text-sm leading-7 text-text-secondary sm:text-[15px]">
          {question.description}
        </p>
      </div>
      <div className="grid gap-4 md:grid-cols-2">
        {question.options.map((option) => {
          const active = option.value === value;
          return (
            <button
              key={option.value}
              type="button"
              onClick={() => onChange(option.value)}
              className={clsx(
                'group rounded-[28px] border px-5 py-5 text-left transition-all duration-200',
                active
                  ? 'border-accent-primary/60 bg-accent-muted/70 shadow-[var(--ui-shadow-2)]'
                  : 'border-border bg-surface-primary/80 hover:border-accent-primary/40 hover:bg-surface-secondary'
              )}
            >
              <div className="flex items-start justify-between gap-4">
                <div className="space-y-2">
                  <div className="text-lg font-semibold text-text-primary">{option.label}</div>
                  <div className="text-sm leading-6 text-text-secondary">{option.description}</div>
                </div>
                <div
                  className={clsx(
                    'mt-1 h-5 w-5 rounded-full border transition-colors',
                    active
                      ? 'border-accent-primary bg-accent-primary ring-4 ring-accent-primary/20'
                      : 'border-border bg-surface-secondary'
                  )}
                />
              </div>
            </button>
          );
        })}
      </div>
    </div>
  );
}

function AbQuestionView({
  question,
  value,
  onChange,
}: {
  question: Extract<RedClawOnboardingQuestion, { type: 'ab' }>;
  value: string;
  onChange: (next: string) => void;
}) {
  return (
    <div className="space-y-8">
      <div className="space-y-3">
        <h2 className="max-w-3xl text-3xl font-semibold leading-tight text-text-primary sm:text-[38px]">
          {question.title}
        </h2>
        <p className="max-w-2xl text-sm leading-7 text-text-secondary sm:text-[15px]">
          {question.description}
        </p>
      </div>
      <div className="grid gap-4 lg:grid-cols-2">
        {question.options.map((option) => {
          const active = option.value === value;
          return (
            <button
              key={option.value}
              type="button"
              onClick={() => onChange(option.value)}
              className={clsx(
                'rounded-[30px] border px-6 py-6 text-left transition-all duration-200',
                active
                  ? 'border-accent-primary/60 bg-accent-muted/70 shadow-[var(--ui-shadow-2)]'
                  : 'border-border bg-surface-primary/80 hover:border-accent-primary/40 hover:bg-surface-secondary'
              )}
            >
              <div className="mb-5 flex items-center justify-between">
                <div className="text-[11px] font-semibold uppercase tracking-[0.24em] text-text-tertiary">
                  选项 {option.label}
                </div>
                <div
                  className={clsx(
                    'h-5 w-5 rounded-full border transition-colors',
                    active
                      ? 'border-accent-primary bg-accent-primary ring-4 ring-accent-primary/20'
                      : 'border-border bg-surface-secondary'
                  )}
                />
              </div>
              <div className="space-y-3 rounded-[22px] border border-border bg-surface-secondary/70 px-5 py-5">
                {option.body.map((line) => (
                  <p key={line} className="text-base leading-7 text-text-primary">
                    {line}
                  </p>
                ))}
              </div>
              <p className="mt-4 text-sm leading-6 text-text-secondary">{option.caption}</p>
            </button>
          );
        })}
      </div>
    </div>
  );
}

export function RedClawOnboardingFlow({
  open,
  activeSpaceName,
  initialStepIndex = 0,
  initialAnswers,
  onClose,
  onSaveProgress,
  onComplete,
}: RedClawOnboardingFlowProps) {
  const [answers, setAnswers] = useState<RedClawOnboardingAnswers>(REDCLAW_ONBOARDING_DEFAULT_ANSWERS);
  const [currentStepIndex, setCurrentStepIndex] = useState(0);
  const [submitting, setSubmitting] = useState(false);
  const [submissionStageIndex, setSubmissionStageIndex] = useState(0);
  const [submissionError, setSubmissionError] = useState('');
  const [hasDefaultModelConfigured, setHasDefaultModelConfigured] = useState(true);
  const [modelConfigMessage, setModelConfigMessage] = useState('');

  useEffect(() => {
    if (!open) return;
    setAnswers(normalizeOnboardingAnswers(initialAnswers));
    setCurrentStepIndex(Math.max(0, Math.min(REDCLAW_ONBOARDING_MVP_QUESTIONS.length - 1, initialStepIndex)));
    setSubmitting(false);
    setSubmissionStageIndex(0);
    setSubmissionError('');
    setModelConfigMessage('');
  }, [initialAnswers, initialStepIndex, open]);

  useEffect(() => {
    if (!open) return;
    let cancelled = false;
    const loadModelConfig = async () => {
      try {
        const settings = await window.ipcRenderer.getSettings();
        if (cancelled) return;
        const defaultModelName = String(settings?.model_name || '').trim();
        const configured = defaultModelName.length > 0;
        setHasDefaultModelConfigured(configured);
        setModelConfigMessage(
          configured
            ? ''
            : '请先在“设置 -> AI 模型”里设置默认模型，再继续风格初始化。没有默认模型，商媒运营助手无法完成后续档案和技能生成。'
        );
      } catch (error) {
        if (cancelled) return;
        console.error('Failed to load settings for Cohmira onboarding:', error);
        setHasDefaultModelConfigured(false);
        setModelConfigMessage('当前无法读取模型配置，请先在“设置 -> AI 模型”确认默认模型已设置，再重新打开风格初始化。');
      }
    };
    void loadModelConfig();
    return () => {
      cancelled = true;
    };
  }, [open]);

  useEffect(() => {
    if (!submitting) return;
    const timer = window.setInterval(() => {
      setSubmissionStageIndex((prev) => Math.min(COMPLETION_STAGES.length - 1, prev + 1));
    }, 900);
    return () => {
      window.clearInterval(timer);
    };
  }, [submitting]);

  const currentQuestion = REDCLAW_ONBOARDING_MVP_QUESTIONS[currentStepIndex];
  const isLastStep = currentStepIndex >= REDCLAW_ONBOARDING_MVP_QUESTIONS.length - 1;
  const blockProgression = false;
  const updateAnswer = (
    key: keyof RedClawOnboardingAnswers,
    nextValue: number | string,
  ) => {
    setAnswers((prev) => ({ ...prev, [key]: nextValue } as RedClawOnboardingAnswers));
  };

  const currentValue = useMemo(() => {
    return answers[currentQuestion.id];
  }, [answers, currentQuestion.id]);

  const commitProgress = async (nextStepIndex: number) => {
    await onSaveProgress({
      stepIndex: Math.max(0, Math.min(REDCLAW_ONBOARDING_MVP_QUESTIONS.length - 1, nextStepIndex)),
      answers,
    });
  };

  const handlePrevious = async () => {
    if (submitting || currentStepIndex <= 0) return;
    const nextStepIndex = currentStepIndex - 1;
    setCurrentStepIndex(nextStepIndex);
    await commitProgress(nextStepIndex);
  };

  const handleNext = async () => {
    if (submitting || blockProgression) return;
    if (isLastStep) {
      setSubmitting(true);
      setSubmissionStageIndex(0);
      setSubmissionError('');
      try {
        await onComplete(answers);
      } catch (error) {
        console.error('Failed to complete Cohmira onboarding:', error);
        setSubmissionError('风格初始化失败，请重试。');
      } finally {
        setSubmitting(false);
      }
      return;
    }
    const nextStepIndex = currentStepIndex + 1;
    setCurrentStepIndex(nextStepIndex);
    await commitProgress(nextStepIndex);
  };

  const handleClose = async () => {
    if (submitting) return;
    setSubmitting(true);
    try {
      await commitProgress(currentStepIndex);
      onClose();
    } finally {
      setSubmitting(false);
    }
  };

  if (!open) return null;

  return (
    <div className="absolute inset-0 z-40 overflow-hidden bg-background">
      <div className="absolute inset-0 bg-[radial-gradient(circle_at_top_left,rgb(var(--color-status-warning)/0.12),transparent_34%),radial-gradient(circle_at_top_right,rgb(var(--color-accent-primary)/0.1),transparent_28%),linear-gradient(180deg,rgb(var(--color-background)/1)_0%,rgb(var(--color-surface-secondary)/0.9)_100%)]" />
      <div className="relative flex h-full min-h-0 flex-col">
        <div className="mx-auto flex w-full max-w-6xl items-center justify-between px-6 pb-4 pt-6 sm:px-8">
          <div className="space-y-1">
            <div className="text-[11px] font-semibold uppercase tracking-[0.24em] text-text-secondary">
              商媒运营助手 · {activeSpaceName || '当前空间'}
            </div>
            <div className="text-lg font-semibold text-text-primary">定义这个空间的经营方向和写作风格</div>
          </div>
          <button
            type="button"
            onClick={() => void handleClose()}
            disabled={submitting}
            className="inline-flex h-10 w-10 items-center justify-center rounded-full border border-border bg-surface-primary/80 text-text-secondary transition hover:bg-surface-secondary hover:text-text-primary disabled:cursor-not-allowed disabled:opacity-35"
            aria-label="关闭"
          >
            <X className="h-4 w-4" />
          </button>
        </div>

        <div className="mx-auto w-full max-w-6xl px-6 pb-4 sm:px-8">
          <QuestionProgress currentStepIndex={currentStepIndex} submitting={submitting} />
        </div>

        <div className="mx-auto flex w-full max-w-6xl min-h-0 flex-1 flex-col px-6 pb-6 sm:px-8">
          <div className="flex-1 overflow-y-auto rounded-[32px] border border-border bg-surface-primary/80 px-6 py-6 shadow-[var(--ui-shadow-2)] backdrop-blur-xl sm:px-8 sm:py-8">
            {submitting ? (
              <CompletionView activeSpaceName={activeSpaceName} stageIndex={submissionStageIndex} />
            ) : (
              <div className="space-y-6">
                {blockProgression ? (
                  <div className="flex items-start gap-3 rounded-2xl border border-status-warning/30 bg-status-warning/10 px-4 py-3 text-sm leading-6 text-text-primary">
                    <AlertCircle className="mt-0.5 h-4 w-4 shrink-0 text-status-warning" />
                    <div>
                      {modelConfigMessage || '请先在“设置 -> AI 模型”里设置默认模型，再继续风格初始化。'}
                    </div>
                  </div>
                ) : null}
                {currentQuestion.type === 'slider' ? (
                  <SliderQuestionView
                    question={currentQuestion}
                    value={Number(currentValue)}
                    onChange={(next) => updateAnswer(currentQuestion.id, next)}
                  />
                ) : currentQuestion.type === 'choice' ? (
                  <ChoiceQuestionView
                    question={currentQuestion}
                    value={String(currentValue)}
                    onChange={(next) => updateAnswer(currentQuestion.id, next)}
                  />
                ) : (
                  <AbQuestionView
                    question={currentQuestion}
                    value={String(currentValue)}
                    onChange={(next) => updateAnswer(currentQuestion.id, next)}
                  />
                )}
              </div>
            )}
          </div>

          {submissionError ? (
            <div className="mt-4 rounded-2xl border border-status-error/30 bg-status-error/10 px-4 py-3 text-sm text-text-primary">
              {submissionError}
            </div>
          ) : null}

          {!submitting ? (
            <div className="mt-5 flex items-center justify-between gap-4">
              <button
                type="button"
                onClick={() => void handlePrevious()}
                disabled={submitting || currentStepIndex <= 0}
                className="inline-flex items-center gap-2 rounded-full border border-border bg-surface-primary/80 px-4 py-2.5 text-sm font-medium text-text-secondary transition hover:bg-surface-secondary hover:text-text-primary disabled:cursor-not-allowed disabled:opacity-35"
              >
                <ArrowLeft className="h-4 w-4" />
                上一题
              </button>
              <button
                type="button"
                onClick={() => void handleNext()}
                disabled={submitting || blockProgression}
                className="inline-flex items-center gap-2 rounded-full bg-accent-primary px-5 py-2.5 text-sm font-semibold text-on-accent transition hover:scale-[0.99] hover:bg-accent-hover disabled:cursor-not-allowed disabled:opacity-60"
              >
                {isLastStep ? '完成并应用' : '下一题'}
                <ArrowRight className="h-4 w-4" />
              </button>
            </div>
          ) : null}
        </div>
      </div>
    </div>
  );
}
