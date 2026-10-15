fn main() {
tauri_build::build();
    embed_common_controls_v6();
}

/// 给本包的链接补一份声明 common-controls v6 的 manifest，否则**单测二进制**在这台
/// 机器上起不来。
///
/// 为什么需要：lib 里的 `import_works_package_impl` 会走「送回收站」
/// （`recycle_to_bin` → `windows` crate 的 `Win32_UI_Shell::SHFileOperationW`）。
/// 这是整个 lib 里第一次有单测触到这条链，于是链接器把
/// `comctl32!TaskDialogIndirect` 静态导进测试二进制。该导出只在 common-controls
/// **v6** 的 comctl32 里存在（WinSxS 那份），System32 那份是 v5 —— 没有 manifest
/// 声明 v6 依赖，Windows 就只加载 v5，测试二进制一启动就
/// `STATUS_ENTRYPOINT_NOT_FOUND` (0xC0000139)。
///
/// app 不受影响：tauri-build 已经给它生成了同一份声明
/// （`target/debug/collection-library.exe` 里能搜到
/// `Microsoft.Windows.Common-Controls`）。重复声明对 MSVC link 是幂等合并。
///
/// 为什么用 `rustc-link-arg` 而不是 `rustc-link-arg-tests`：后者只作用于 `tests/`
/// 目录下的集成测试目标，管不到 lib 内的 `#[cfg(test)]` 单测。
/// 为什么不用 `.cargo/config.toml` 的 `[target.*.rustflags]`：那是全局的，会让所有
/// 依赖的 build script 链接时也带上这两个参数，而它们的链接时工作目录不是包根，
/// 相对路径的 `/MANIFESTINPUT:` 直接解析失败（link.exe exit 1327）。
fn embed_common_controls_v6() {
    let Ok(manifest_dir) = std::env::var("CARGO_MANIFEST_DIR") else {
        return;
    };
    let manifest = std::path::Path::new(&manifest_dir).join("tests-common-controls-v6.manifest");
    if !manifest.is_file() {
        return;
    }
    if !cfg!(target_os = "windows") {
        return;
    }
    // 方案：用 `/DELAYLOAD:comctl32.dll` 把 comctl32 的导入改成**延迟加载**。
    //
    // 为什么这样更稳：manifest 那条路走不通 ——
    //  - `rustc-link-arg-tests` 只管 `tests/` 目录的集成测试，管不到 lib 内的单测；
    //  - `rustc-link-arg`（全局）会连 bin 目标一起带上，而 bin（app）已经由
    //    tauri-build 嵌了一份同名 manifest，link.exe 报 `CVT1100 资源重复` 失败。
    //
    // 而 `TaskDialogIndirect` 只在 common-controls v6 的 comctl32 里导出，System32
    // 那份 v5 没有。延迟加载后，加载器启动时不去解析这些入口，只有真调用到才解析；
    // 单测跑的是文件搬运和回收站，从不弹任务对话框，所以永远解析不到 ⇒ 能启动。
    //
    // 对 app 无副作用：bin 目标本来能跑，延迟加载只会把 comctl32 的解析挪到首次调用。
    println!("cargo:rustc-link-arg=/DELAYLOAD:comctl32.dll");
    // 延迟加载需要 delayimp.lib 提供 __delayLoadHelper2，否则 LNK1120。
    println!("cargo:rustc-link-lib=delayimp");
    println!("cargo:rerun-if-changed=tests-common-controls-v6.manifest");
}