use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

fn main() {
    println!("cargo:rerun-if-changed=assets/app-icon.ico");
    println!("cargo:rerun-if-changed=build.rs");

    if env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }

    let manifest_dir = PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").unwrap());
    let icon_path = manifest_dir.join("assets/app-icon.ico");
    let out_dir = PathBuf::from(env::var_os("OUT_DIR").unwrap());
    let rc_path = out_dir.join("app-icon.rc");
    let resource_path = out_dir.join("app-icon.res");

    let normalized_icon_path = icon_path.to_string_lossy().replace('\\', "/");
    fs::write(&rc_path, format!("1 ICON \"{normalized_icon_path}\"\n"))
        .expect("failed to write Windows icon resource script");

    let compiler =
        find_resource_compiler().expect("Windows SDK resource compiler rc.exe was not found");
    let status = Command::new(&compiler)
        .arg("/nologo")
        .arg(format!("/fo{}", resource_path.display()))
        .arg(&rc_path)
        .status()
        .expect("failed to run Windows SDK resource compiler");
    assert!(status.success(), "rc.exe failed with status {status}");

    println!(
        "cargo:rustc-link-arg-bin=industry-excel-checker={}",
        resource_path.display()
    );
}

fn find_resource_compiler() -> Option<PathBuf> {
    if let Some(path) = env::var_os("RC").map(PathBuf::from)
        && path.is_file()
    {
        return Some(path);
    }

    if Command::new("rc.exe").arg("/?").output().is_ok() {
        return Some(PathBuf::from("rc.exe"));
    }

    let program_files = env::var_os("ProgramFiles(x86)")?;
    let bin_dir = Path::new(&program_files).join("Windows Kits/10/bin");
    let mut versions = fs::read_dir(bin_dir)
        .ok()?
        .filter_map(Result::ok)
        .filter(|entry| entry.file_type().is_ok_and(|file_type| file_type.is_dir()))
        .collect::<Vec<_>>();
    versions.sort_by_key(|entry| std::cmp::Reverse(entry.file_name()));

    versions
        .into_iter()
        .map(|entry| entry.path().join("x64/rc.exe"))
        .find(|candidate| candidate.is_file())
}
