//! 商媒运营助手 live e2e：用 config.json 的真实 key 驱动 Goose，跑 8 个模拟用户任务。
//!
//! 用法：
//! ```bash
//! cd src
//! cargo run -p yunying-server --bin e2e_operations            # 跑全部 8 个
//! cargo run -p yunying-server --bin e2e_operations -- baby_hotspots  # 只跑一个
//! ```
//!
//! 每个 scenario 的用户请求发给嵌入式 Goose，收集模型回复文本，写入 `work-packages/<id>.md`。
//! 验证「聊天页接 goose」的完整链路：config.json → GooseBridge(provider+agent+session) → reply 流式 → 落盘。
//!
//! 注：模型的 operations 工具调用（start_task/upload_video 等 → 产出结构化 workParams）需要
//! yunying-ops 作为 Goose MCP 扩展注册（下一步集成）。本 e2e 当前验证 LLM 回复链路。

use futures::StreamExt;
use yunying_server::goose_bridge::{BridgeEvent, GooseBridge};
use yunying_server::ipc::{dispatch_invoke, AppState, NoopEmitter};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    if std::env::var_os("GOOSE_PATH_ROOT").is_none() {
        let root = std::env::current_dir()?.join("target/e2e-goose");
        std::fs::create_dir_all(&root)?;
        std::env::set_var("GOOSE_PATH_ROOT", root);
    }
    let only = std::env::args().nth(1);
    let cfg = yunying_config::load(None)?;
    println!(
        "九伴 e2e：Goose provider={} model={} base={}",
        cfg.goose.provider, cfg.goose.model, cfg.goose.base_url
    );
    if cfg.goose.api_key.is_empty() {
        anyhow::bail!("config.json 缺 goose.api_key（填入真实 key 后重试）");
    }

    let bridge = GooseBridge::new(&cfg).await?;
    let _e2e_bridge_task = if only.as_deref() == Some("image-tool-real-smoke") {
        Some(start_e2e_plugin_bridge(&bridge).await?)
    } else {
        None
    };

    // 注册 yunying-ops-mcp：让模型发现并自主调用 operations 工具（产出结构化 workParams）。
    // 环境变量 YUNYING_OPS_MCP 可覆盖 bin 路径；默认 dev 构建路径。
    let ops_bin =
        std::env::var("YUNYING_OPS_MCP").unwrap_or_else(|_| "target/debug/yunying-ops-mcp".into());
    if std::path::Path::new(&ops_bin).exists() {
        match bridge.register_operations_mcp(&ops_bin).await {
            Ok(()) => println!("已注册 operations MCP：{ops_bin}（模型可调用 operations 工具）"),
            Err(e) => println!("[warn] 注册 operations MCP 失败（模型将纯文本回复）：{e}"),
        }
    } else {
        println!("[info] 未找到 {ops_bin}，跳过 operations MCP 注册（模型纯文本回复）。用 `cargo build -p yunying-ops --features mcp --bin yunying-ops-mcp` 构建后启用。");
    }

    if only.as_deref() == Some("smoke") {
        run_smoke(&bridge).await?;
        return Ok(());
    }
    if only.as_deref() == Some("tool-smoke") {
        run_tool_smoke(&bridge).await?;
        return Ok(());
    }
    if only.as_deref() == Some("image-tool-smoke") {
        run_image_tool_smoke(&bridge).await?;
        return Ok(());
    }
    if only.as_deref() == Some("image-tool-real-smoke") {
        run_image_tool_real_smoke(&bridge).await?;
        return Ok(());
    }
    if only.as_deref() == Some("video-generation-smoke") {
        run_video_generation_smoke(&bridge).await?;
        return Ok(());
    }

    let out_dir = std::path::Path::new("work-packages");
    std::fs::create_dir_all(out_dir)?;

    let scenarios = yunying_ops::scenarios::all();
    let total = if only.is_some() { 1 } else { scenarios.len() };
    println!("共 {} 个任务\n", total);

    let mut done = 0;
    let mut ok = 0;
    for s in &scenarios {
        if let Some(id) = &only {
            if s.id != id {
                continue;
            }
        }
        done += 1;
        println!("[{}/{}] {}：{}", done, total, s.id, s.user_request);
        match run_one(&bridge, s.id, s.user_request, out_dir).await {
            Ok(chars) => {
                println!("  ✓ work-packages/{}.md（{} 字符）", s.id, chars);
                ok += 1;
            }
            Err(e) => {
                println!("  ✗ 失败：{}", e);
            }
        }
    }

    println!("\n完成：{ok}/{done} 成功");
    if ok == 0 {
        std::process::exit(1);
    }
    Ok(())
}

fn app_data_dir() -> anyhow::Result<std::path::PathBuf> {
    let home = std::env::var_os("HOME")
        .map(std::path::PathBuf::from)
        .ok_or_else(|| anyhow::anyhow!("HOME is not set"))?;
    Ok(std::env::var_os("YUNYING_E2E_APP_DATA_DIR")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| {
            home.join("Library/Application Support/com.agentscompany.yunyingagent.desktop")
        }))
}

fn e2e_app_state(bridge: &GooseBridge) -> anyhow::Result<AppState> {
    let db_path = app_data_dir()?.join("redconvert.db");
    if !db_path.is_file() {
        anyhow::bail!("应用数据库不存在：{}", db_path.display());
    }
    let db = yunying_server::db::Db::open(&db_path)?;
    Ok(AppState {
        redclaw_scheduler: yunying_server::ipc::redclaw_runner::RedClawScheduler::inactive(
            db.clone(),
        ),
        db,
        goose: bridge.clone(),
        emitter: std::sync::Arc::new(NoopEmitter),
        login: std::sync::Arc::new(yunying_server::login::LoginService::new(
            std::sync::Arc::new(yunying_server::login::StubLoginDriver),
        )),
    })
}

async fn start_e2e_plugin_bridge(
    bridge: &GooseBridge,
) -> anyhow::Result<tokio::task::JoinHandle<()>> {
    let server = yunying_server::plugins::bridge::PluginBridgeServer::bind_loopback().await?;
    let endpoint = server.endpoint().clone();
    endpoint.install_process_environment();
    let address = endpoint.address();
    let state = std::sync::Arc::new(e2e_app_state(bridge)?);
    let task = tokio::spawn(async move {
        if let Err(error) = server.serve(state).await {
            eprintln!("[e2e] 本地插件桥失败：{error}");
        }
    });
    eprintln!("[e2e] 本地插件桥已绑定 {address}");
    Ok(task)
}

async fn run_smoke(bridge: &GooseBridge) -> anyhow::Result<()> {
    let started = std::time::Instant::now();
    let stream = bridge
        .reply("这是连通性测试。不要调用任何工具，只回复：OK")
        .await?;
    let mut text = String::new();
    let mut stream_error = None;
    let mut first_text_ms = None;
    tokio::pin!(stream);
    while let Some(event) = stream.next().await {
        match event {
            BridgeEvent::TextDelta(delta) => {
                if first_text_ms.is_none() {
                    first_text_ms = Some(started.elapsed().as_millis());
                }
                text.push_str(&delta);
            }
            BridgeEvent::Error {
                message,
                detail,
                category,
            } => stream_error = Some(format!("{message} [{category}]: {detail}")),
            BridgeEvent::ThoughtDelta(_)
            | BridgeEvent::ToolStart { .. }
            | BridgeEvent::ToolEnd { .. }
            | BridgeEvent::Cancelled
            | BridgeEvent::Done => {}
        }
    }
    let total_ms = started.elapsed().as_millis();
    if let Some(error) = stream_error {
        anyhow::bail!("模型流失败；总耗时 {total_ms}ms；错误={error}");
    }
    if text.trim().is_empty() {
        anyhow::bail!("模型未返回文本；总耗时 {total_ms}ms");
    }
    let normalized = text.to_lowercase();
    if normalized.contains("network error:")
        || normalized.contains("could not connect")
        || normalized.contains("please resend your message")
    {
        anyhow::bail!("模型连接失败；总耗时 {total_ms}ms；回复={}", text.trim());
    }
    println!(
        "smoke OK：首文本={}ms，总耗时={}ms，回复={}",
        first_text_ms.unwrap_or(total_ms),
        total_ms,
        text.trim()
    );
    Ok(())
}

async fn run_tool_smoke(bridge: &GooseBridge) -> anyhow::Result<()> {
    let started = std::time::Instant::now();
    let stream = bridge
        .reply(
            "这是工具协议回归测试。必须调用 yunying-ops 的 list_capabilities 工具一次，然后用一句中文总结工具结果。",
        )
        .await?;
    let mut starts = Vec::new();
    let mut ends = Vec::new();
    let mut text = String::new();
    let mut stream_error = None;
    tokio::pin!(stream);
    while let Some(event) = stream.next().await {
        match event {
            BridgeEvent::TextDelta(delta) => text.push_str(&delta),
            BridgeEvent::ToolStart { name, .. } => starts.push(name),
            BridgeEvent::ToolEnd { name, .. } => ends.push(name),
            BridgeEvent::Error {
                message,
                detail,
                category,
            } => stream_error = Some(format!("{message} [{category}]: {detail}")),
            BridgeEvent::ThoughtDelta(_) | BridgeEvent::Cancelled | BridgeEvent::Done => {}
        }
    }
    if let Some(error) = stream_error {
        anyhow::bail!("工具流失败：{error}");
    }
    if starts.is_empty() || ends.is_empty() {
        anyhow::bail!(
            "模型没有完成工具调用；starts={starts:?} ends={ends:?} reply={}",
            text.trim()
        );
    }
    if starts
        .iter()
        .any(|name| !name.contains("list_capabilities"))
    {
        anyhow::bail!("模型调用了非预期工具：{starts:?}");
    }
    println!(
        "tool-smoke OK：工具开始={starts:?}，工具结束={ends:?}，总耗时={}ms，回复={}",
        started.elapsed().as_millis(),
        text.trim()
    );
    Ok(())
}

async fn run_image_tool_smoke(bridge: &GooseBridge) -> anyhow::Result<()> {
    let started = std::time::Instant::now();
    let stream = bridge
        .reply(
            "这是图片工具协议回归测试。必须调用 yunying-ops 的 generate_image 工具一次，参数为：prompt='温馨儿童房中的原木儿童学习桌，写实产品摄影，无文字'、aspect_ratio='3:4'、count=1、dry_run=true。不要调用其他工具，然后用一句中文总结计划结果。",
        )
        .await?;
    let mut starts = Vec::new();
    let mut ends = Vec::new();
    let mut outputs = Vec::new();
    let mut text = String::new();
    let mut stream_error = None;
    tokio::pin!(stream);
    while let Some(event) = stream.next().await {
        match event {
            BridgeEvent::TextDelta(delta) => text.push_str(&delta),
            BridgeEvent::ToolStart { name, input, .. } => starts.push((name, input)),
            BridgeEvent::ToolEnd { name, output, .. } => {
                ends.push(name);
                outputs.push(output);
            }
            BridgeEvent::Error {
                message,
                detail,
                category,
            } => stream_error = Some(format!("{message} [{category}]: {detail}")),
            BridgeEvent::ThoughtDelta(_) | BridgeEvent::Cancelled | BridgeEvent::Done => {}
        }
    }
    if let Some(error) = stream_error {
        anyhow::bail!("图片工具流失败：{error}");
    }
    if starts.len() != 1 || ends.len() != 1 {
        anyhow::bail!(
            "图片工具调用次数异常；starts={starts:?} ends={ends:?} reply={}",
            text.trim()
        );
    }
    if !starts[0].0.contains("generate_image") || !ends[0].contains("generate_image") {
        anyhow::bail!("模型调用了非预期工具；starts={starts:?} ends={ends:?}");
    }
    if starts[0]
        .1
        .get("dry_run")
        .and_then(serde_json::Value::as_bool)
        != Some(true)
    {
        anyhow::bail!("图片工具没有保持 dry_run=true：{}", starts[0].1);
    }
    let output_text = serde_json::to_string(&outputs).unwrap_or_default();
    if !output_text.contains("plannedAction") || !output_text.contains("gpt-image-2") {
        anyhow::bail!("图片工具未返回预期计划：{output_text}");
    }
    println!(
        "image-tool-smoke OK：工具={}，总耗时={}ms，回复={}",
        starts[0].0,
        started.elapsed().as_millis(),
        text.trim()
    );
    Ok(())
}

fn first_existing_asset_path(value: &serde_json::Value) -> Option<std::path::PathBuf> {
    match value {
        serde_json::Value::Object(map) => {
            if let Some(path) = map.get("absolutePath").and_then(serde_json::Value::as_str) {
                let path = std::path::PathBuf::from(path);
                if path.is_file() {
                    return Some(path);
                }
            }
            map.values().find_map(first_existing_asset_path)
        }
        serde_json::Value::Array(items) => items.iter().find_map(first_existing_asset_path),
        serde_json::Value::String(text) => {
            let text = text.trim();
            if (text.starts_with('{') || text.starts_with('[')) && text.len() <= 2_000_000 {
                serde_json::from_str(text)
                    .ok()
                    .and_then(|parsed| first_existing_asset_path(&parsed))
            } else {
                None
            }
        }
        _ => None,
    }
}

async fn run_image_tool_real_smoke(bridge: &GooseBridge) -> anyhow::Result<()> {
    let started = std::time::Instant::now();
    let stream = bridge
        .reply(
            "这是图片生成真实端到端测试。必须调用 yunying-ops 的 generate_image 工具一次，参数为：prompt='温馨明亮的儿童房，浅木色儿童学习桌，真实室内产品摄影，自然光，无人物，无文字，无水印'、aspect_ratio='3:4'、count=1、dry_run=false。不要调用其他工具；等待工具完成后只用一句中文说明生成文件。",
        )
        .await?;
    let mut starts = Vec::new();
    let mut ends = Vec::new();
    let mut outputs = Vec::new();
    let mut text = String::new();
    let mut stream_error = None;
    tokio::pin!(stream);
    while let Some(event) = stream.next().await {
        match event {
            BridgeEvent::TextDelta(delta) => text.push_str(&delta),
            BridgeEvent::ToolStart { name, input, .. } => starts.push((name, input)),
            BridgeEvent::ToolEnd { name, output, .. } => {
                ends.push(name);
                outputs.push(output);
            }
            BridgeEvent::Error {
                message,
                detail,
                category,
            } => stream_error = Some(format!("{message} [{category}]: {detail}")),
            BridgeEvent::ThoughtDelta(_) | BridgeEvent::Cancelled | BridgeEvent::Done => {}
        }
    }
    if let Some(error) = stream_error {
        anyhow::bail!("真实图片工具流失败：{error}");
    }
    if starts.len() != 1 || ends.len() != 1 {
        anyhow::bail!(
            "真实图片工具调用次数异常；starts={starts:?} ends={ends:?} reply={}",
            text.trim()
        );
    }
    if starts[0]
        .1
        .get("dry_run")
        .and_then(serde_json::Value::as_bool)
        != Some(false)
    {
        anyhow::bail!("真实图片工具没有保持 dry_run=false：{}", starts[0].1);
    }
    let asset_path = outputs
        .iter()
        .find_map(first_existing_asset_path)
        .ok_or_else(|| anyhow::anyhow!("真实图片工具没有返回可用素材：{outputs:?}"))?;
    let metadata = std::fs::metadata(&asset_path)?;
    if metadata.len() == 0 {
        anyhow::bail!("真实图片素材为空：{}", asset_path.display());
    }
    println!(
        "image-tool-real-smoke OK：耗时={}ms，文件={}，大小={} bytes，回复={}",
        started.elapsed().as_millis(),
        asset_path.display(),
        metadata.len(),
        text.trim()
    );
    Ok(())
}

async fn run_video_generation_smoke(bridge: &GooseBridge) -> anyhow::Result<()> {
    let state = e2e_app_state(bridge)?;
    let started = std::time::Instant::now();
    let result = dispatch_invoke(
        "video-gen:generate",
        serde_json::json!({
            "prompt": "一张原木儿童学习桌置于明亮温馨的儿童房，镜头缓慢推进，写实产品广告，无文字",
            "title": "九伴视频生成端到端回归测试",
            "count": 1,
            "aspectRatio": "16:9",
            "resolution": "720p",
            "durationSeconds": 5,
            "generationMode": "text-to-video"
        }),
        &state,
    )
    .await?;
    if result.get("success").and_then(serde_json::Value::as_bool) != Some(true) {
        anyhow::bail!("视频生成失败：{}", result);
    }
    let asset = result
        .get("assets")
        .and_then(serde_json::Value::as_array)
        .and_then(|assets| assets.first())
        .ok_or_else(|| anyhow::anyhow!("视频生成成功但没有返回素材：{}", result))?;
    let absolute_path = asset
        .get("absolutePath")
        .and_then(serde_json::Value::as_str)
        .map(std::path::PathBuf::from)
        .ok_or_else(|| anyhow::anyhow!("视频素材缺少 absolutePath：{}", asset))?;
    let metadata = std::fs::metadata(&absolute_path)?;
    if !metadata.is_file() || metadata.len() == 0 {
        anyhow::bail!("视频素材为空：{}", absolute_path.display());
    }
    println!(
        "video-generation-smoke OK：耗时={}ms，文件={}，大小={} bytes，模型={}",
        started.elapsed().as_millis(),
        absolute_path.display(),
        metadata.len(),
        asset
            .get("model")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("unknown")
    );
    Ok(())
}

async fn run_one(
    bridge: &GooseBridge,
    id: &str,
    request: &str,
    out_dir: &std::path::Path,
) -> anyhow::Result<usize> {
    let stream = bridge.reply(request).await?;
    let mut text = String::new();
    let mut stream_error = None;
    tokio::pin!(stream);
    while let Some(ev) = stream.next().await {
        match ev {
            BridgeEvent::TextDelta(t) => text.push_str(&t),
            BridgeEvent::Error {
                message,
                detail,
                category,
            } => stream_error = Some(format!("{message} [{category}]: {detail}")),
            BridgeEvent::ThoughtDelta(_)
            | BridgeEvent::ToolStart { .. }
            | BridgeEvent::ToolEnd { .. }
            | BridgeEvent::Cancelled
            | BridgeEvent::Done => {}
        }
    }
    if let Some(error) = stream_error {
        anyhow::bail!("模型流失败：{error}");
    }
    let path = out_dir.join(format!("{id}.md"));
    std::fs::write(
        &path,
        format!("# {id}\n\n**请求**：{request}\n\n**模型回复**：\n\n{text}\n"),
    )?;
    Ok(text.chars().count())
}
