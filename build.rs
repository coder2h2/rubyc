use std::env;
use std::path::PathBuf;
use std::process::Command;

fn main() {
    println!("cargo:rerun-if-changed=runtime/runtime.c");
    println!("cargo:rerun-if-changed=runtime/runtime.h");

    let out_dir = PathBuf::from(env::var("OUT_DIR").unwrap());
    let obj_path = out_dir.join("runtime.o");
    let lib_path = out_dir.join("librubycom_rt.a");

    let target = env::var("TARGET").unwrap_or_default();
    let host = env::var("HOST").unwrap_or_default();
    let (cc, ar) = if target != host && target.starts_with("aarch64") {
        ("aarch64-linux-gnu-gcc", "aarch64-linux-gnu-ar")
    } else {
        ("gcc", "ar")
    };

    let status = Command::new(cc)
        .args([
            "-O2",
            "-fPIC",
            "-Wall",
            "-Wextra",
            "-Wno-unused-parameter",
            "-c",
            "runtime/runtime.c",
            "-o",
        ])
        .arg(&obj_path)
        .status()
        .expect("failed to invoke gcc to compile runtime.c");
    assert!(status.success(), "gcc failed compiling runtime/runtime.c");

    let ar_status = Command::new(ar)
        .args(["rcs"])
        .arg(&lib_path)
        .arg(&obj_path)
        .status()
        .expect("failed to invoke ar");
    assert!(ar_status.success(), "ar failed creating librubycom_rt.a");

    println!(
        "cargo:rustc-env=RUBYCOM_RT_LIB={}",
        lib_path.display()
    );
}
