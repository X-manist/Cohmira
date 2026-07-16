use std::env;
use std::fs;
use std::path::{Path, PathBuf};

fn find_on_path(executable: &str) -> Option<PathBuf> {
    let path = env::var_os("PATH")?;
    env::split_paths(&path)
        .map(|directory| directory.join(executable))
        .find(|candidate| candidate.is_file())
}

fn prepare_uv_resource(manifest_dir: &Path) {
    println!("cargo:rerun-if-env-changed=UV_BIN");
    println!("cargo:rerun-if-env-changed=PATH");
    let executable_name = if cfg!(windows) { "uv.exe" } else { "uv" };
    let source = env::var_os("UV_BIN")
        .map(PathBuf::from)
        .filter(|path| path.is_file())
        .or_else(|| find_on_path(executable_name));
    let Some(source) = source else {
        println!(
            "cargo:warning=未找到 {executable_name}；将只打包自包含 Python 运行时，不提供开发态 uv 后备"
        );
        return;
    };
    let target_dir = manifest_dir.join("runtime").join("bin");
    fs::create_dir_all(&target_dir).expect("创建 Tauri uv 资源目录失败");
    let target = target_dir.join(executable_name);
    fs::copy(&source, &target).unwrap_or_else(|error| {
        panic!(
            "复制 uv 到 Tauri 资源目录失败（{} -> {}）：{error}",
            source.display(),
            target.display()
        )
    });
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&target, fs::Permissions::from_mode(0o755))
            .expect("设置 uv 可执行权限失败");
    }
}

fn main() {
    let manifest_dir = PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").unwrap());
    prepare_uv_resource(&manifest_dir);
    tauri_build::build();
}
