mod ast;
mod codegen;
mod lexer;
mod parser;

use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};

use codegen::Codegen;
use parser::parse_source;

const RT_LIB_BYTES: &[u8] = include_bytes!(env!("RUBYCOM_RT_LIB"));

fn print_usage() {
    eprintln!("rubyc - True Ahead-Of-Time Ruby to x86-64 Native Compiler");
    eprintln!("Usage: rubyc <input.rb> [-o <output>] [--emit-asm] [--run] [-- <args>...]");
}

fn main() -> ExitCode {
    let args: Vec<String> = env::args().skip(1).collect();
    if args.is_empty() {
        print_usage();
        return ExitCode::from(1);
    }

    let mut input_path: Option<String> = None;
    let mut output_path: Option<String> = None;
    let mut emit_asm = false;
    let mut run_after = false;
    let mut prog_args: Vec<String> = Vec::new();

    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "-h" | "--help" => {
                print_usage();
                return ExitCode::SUCCESS;
            }
            "-o" => {
                i += 1;
                if i >= args.len() {
                    eprintln!("error: -o requires an argument");
                    return ExitCode::from(1);
                }
                output_path = Some(args[i].clone());
            }
            "--emit-asm" | "-S" => {
                emit_asm = true;
            }
            "--run" | "-r" => {
                run_after = true;
            }
            "--" => {
                prog_args.extend_from_slice(&args[i + 1..]);
                break;
            }
            arg if arg.starts_with('-') => {
                eprintln!("error: unknown option '{}'", arg);
                return ExitCode::from(1);
            }
            arg => {
                if input_path.is_none() {
                    input_path = Some(arg.to_string());
                } else {
                    eprintln!("error: unexpected argument '{}'", arg);
                    return ExitCode::from(1);
                }
            }
        }
        i += 1;
    }

    let Some(input) = input_path else {
        print_usage();
        return ExitCode::from(1);
    };

    let src = match fs::read_to_string(&input) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("error reading '{}': {}", input, e);
            return ExitCode::from(1);
        }
    };

    let prog = match parse_source(&src) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("syntax error in '{}': {}", input, e);
            return ExitCode::from(1);
        }
    };

    let asm = Codegen::new().generate(&prog);

    let stem = Path::new(&input)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("a.out");

    if emit_asm {
        let asm_out = output_path.unwrap_or_else(|| format!("{}.s", stem));
        if let Err(e) = fs::write(&asm_out, &asm) {
            eprintln!("error writing '{}': {}", asm_out, e);
            return ExitCode::from(1);
        }
        return ExitCode::SUCCESS;
    }

    let out_bin = output_path.unwrap_or_else(|| stem.to_string());
    let tmp_dir = env::temp_dir();
    let pid = std::process::id();
    let asm_file: PathBuf = tmp_dir.join(format!("rubycom_{}_{}.s", stem, pid));
    let obj_file: PathBuf = tmp_dir.join(format!("rubycom_{}_{}.o", stem, pid));

    if let Err(e) = fs::write(&asm_file, &asm) {
        eprintln!("error writing temp asm: {}", e);
        return ExitCode::from(1);
    }

    let as_status = Command::new("as")
        .arg(&asm_file)
        .arg("-o")
        .arg(&obj_file)
        .status();

    match as_status {
        Ok(s) if s.success() => {}
        Ok(s) => {
            eprintln!("assembler failed with status {}", s);
            let _ = fs::remove_file(&asm_file);
            return ExitCode::from(1);
        }
        Err(e) => {
            eprintln!("failed to run 'as': {}", e);
            let _ = fs::remove_file(&asm_file);
            return ExitCode::from(1);
        }
    }

    let rt_lib = tmp_dir.join(format!("rubyc_rt_{}_{}.a", stem, pid));
    if let Err(e) = fs::write(&rt_lib, RT_LIB_BYTES) {
        eprintln!("error writing temp runtime lib: {}", e);
        let _ = fs::remove_file(&asm_file);
        let _ = fs::remove_file(&obj_file);
        return ExitCode::from(1);
    }

    let ld_status = Command::new("gcc")
        .arg(&obj_file)
        .arg(&rt_lib)
        .arg("-lm")
        .arg("-o")
        .arg(&out_bin)
        .status();

    let _ = fs::remove_file(&asm_file);
    let _ = fs::remove_file(&obj_file);
    let _ = fs::remove_file(&rt_lib);

    match ld_status {
        Ok(s) if s.success() => {}
        Ok(s) => {
            eprintln!("linker failed with status {}", s);
            return ExitCode::from(1);
        }
        Err(e) => {
            eprintln!("failed to run 'gcc' linker: {}", e);
            return ExitCode::from(1);
        }
    }

    if run_after {
        let bin_cmd = if Path::new(&out_bin).components().count() == 1 {
            format!("./{}", out_bin)
        } else {
            out_bin.clone()
        };
        let status = Command::new(&bin_cmd).args(&prog_args).status();
        match status {
            Ok(s) => {
                let code = s.code().unwrap_or(1) as u8;
                return ExitCode::from(code);
            }
            Err(e) => {
                eprintln!("failed to execute '{}': {}", out_bin, e);
                return ExitCode::from(1);
            }
        }
    }

    ExitCode::SUCCESS
}
