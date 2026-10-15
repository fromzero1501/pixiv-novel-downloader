fn main() {
    tauri_build::build();
    delay_load_comctl32();
}

/// 把 comctl32 的导入改成**延迟加载**，否则**单测二进制**在这台机器上起不来。
///
/// 症状：`cargo test` 报 `STATUS_ENTRYPOINT_NOT_FOUND` (0xC0000139)，测试二进制连
/// `--list` 都跑不了、零输出；而用改动前的源码编译就一切正常，看着像自己代码的锅。
///
/// 真因：lib 里的 `import_works_package_impl` 会走「送回收站」
/// （`recycle_to_bin` → `windows` crate 的 `Win32_UI_Shell::SHFileOperationW`）。
/// 这是整个 lib 里第一次有单测触到这条链，于是链接器把
/// `comctl32!TaskDialogIndirect` 静态导进测试二进制。该导出只在 common-controls
/// **v6** 的 comctl32 里存在（WinSxS 那份），System32 那份是 v5 —— 没有 manifest
/// 声明 v6 依赖，Windows 就只加载 v5，于是启动即入口点缺失。
///
/// app 不受影响：tauri-build 已经给 `collection-library.exe` 生成了同一份声明
/// （`python -c "print(b'Common-Controls' in open('collection-library.exe','rb').read())"`
/// 可自查）。只有不走 tauri-build 的测试目标缺。
///
/// 为什么给 manifest 那条路走不通（都试过）：
///  - `cargo:rustc-link-arg-tests` 只管 `tests/` 目录下的集成测试，管不到 lib 内的
///    `#[cfg(test)]` 单测目标；
///  - 换成全局 `cargo:rustc-link-arg` 会连 bin 一起带，而 bin 已经由 tauri-build
///    嵌了同名 manifest，link.exe 直接报 `CVT1100 资源重复`；
///  - 丢进 `.cargo/config.toml` 的 `[target.*.rustflags]` 更糟：全局生效，所有依赖的
///    build script 链接时也带上，而它们的链接工作目录不是包根，相对路径解析失败
///    （link.exe exit 1327）；`profile.test` 又不支持 `rustflags`。
///
/// 延迟加载为什么对：加载器启动时不去解析 comctl32 那些入口，只有真调用到才解析。
/// 单测跑的是文件搬运和送回收站，从不弹任务对话框，所以永远解析不到 ⇒ 能启动。
/// 对 app 也无副作用 —— 只是把 comctl32 的解析挪到首次调用。
fn delay_load_comctl32() {
    if !cfg!(target_os = "windows") {
        return;
    }
    println!("cargo:rustc-link-arg=/DELAYLOAD:comctl32.dll");
    // 延迟加载要 delayimp.lib 提供 __delayLoadHelper2，漏了这行报 LNK1120。
    println!("cargo:rustc-link-lib=delayimp");
}
