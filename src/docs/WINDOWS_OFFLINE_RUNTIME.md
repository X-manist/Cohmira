# Windows 离线媒体与插件运行时

Windows 安装包内置两个自包含运行时，员工电脑无需安装 FFmpeg、Python、pip 或 uv，
也不会在插件启动时访问 PyPI：

- `ffmpeg-runtime/`：固定版本的 BtbN Windows x64 GPL static 完整发行包，包含
  `ffmpeg.exe`、`ffprobe.exe`、`ffplay.exe`、预设和许可证文件。
- `python-runtime/`：CPython 3.11.9 embeddable x64，加上
  `builtin-plugins/openmontage/requirements-windows.lock` 中的全部 wheel。

## 在 Windows 构建机准备

构建机需要 Node.js 22、PowerShell 和 uv。运行：

```powershell
node scripts/prepare-windows-runtime.mjs
node scripts/verify-windows-runtime.mjs
```

`prepare-tauri-build.mjs` 在 Windows release 构建时会自动执行同一准备步骤。下载文件缓存在
`.runtime-cache/windows`（包括上游压缩包和 uv wheel 缓存）；也可设置 `JIUBAN_RUNTIME_CACHE`，或传入
`--cache-dir D:\runtime-cache`。缓存中存在且 SHA-256 正确时不会联网，因此可先把缓存目录
带入隔离构建环境再打包。

需要重建时使用：

```powershell
node scripts/prepare-windows-runtime.mjs --force
```

脚本会在结束前实际运行 Python import smoke test，以及 `ffmpeg -version` 和
`ffprobe -version`。生成的 `src-tauri/runtime/windows-runtime-manifest.json` 记录固定来源、
版本、SHA-256 和入口路径。Tauri 资源表与自定义 NSIS 安装器都会携带这三个目录/清单。

## 运行时路径

应用启动时设置以下环境变量，插件直接用内置 Python 启动 MCP，不经过在线依赖解析：

- `JIUBAN_PYTHON_BIN=<resources>\python-runtime\python.exe`
- `PYTHONHOME=<resources>\python-runtime`
- `JIUBAN_FFMPEG_DIR=<resources>\ffmpeg-runtime`
- `FFMPEG_BIN=<resources>\ffmpeg-runtime\ffmpeg.exe`
- `FFPROBE_BIN=<resources>\ffmpeg-runtime\ffprobe.exe`

uv 仅保留为开发环境后备，不是 Windows 安装包运行 Python 插件的前置条件。
