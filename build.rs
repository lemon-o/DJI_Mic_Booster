use std::path::Path;
use std::process::Command;

fn main() {
    println!("cargo:rerun-if-changed=app.rc");
    println!("cargo:rerun-if-changed=app.ico");
    println!("cargo:rerun-if-changed=app.manifest");

    let out_dir = std::env::var("OUT_DIR").unwrap();
    let res_path = format!("{}/app.res", out_dir);

    // 优先使用 Windows SDK 内置的 rc.exe
    let rc_paths = [
        "C:\\Program Files (x86)\\Windows Kits\\10\\bin\\10.0.19041.0\\x64\\rc.exe",
        "C:\\Program Files (x86)\\Windows Kits\\10\\bin\\10.0.22000.0\\x64\\rc.exe",
        "C:\\Program Files (x86)\\Windows Kits\\10\\bin\\10.0.22621.0\\x64\\rc.exe",
        "rc.exe",
    ];

    for rc in rc_paths {
        if Path::new(rc).exists() || rc == "rc.exe" {
            let status = Command::new(rc)
                .args(&["/c", "65001", "/fo", &res_path, "app.rc"])
                .status();
            if let Ok(s) = status {
                if s.success() {
                    println!("cargo:rustc-link-arg={}", res_path);
                    break;
                }
            }
        }
    }
}
