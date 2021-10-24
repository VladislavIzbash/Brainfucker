use std::error::Error;
use std::fmt::{Display, Formatter};
use std::fs;
use std::path::Path;
use std::process::Command;

use anyhow::Context as _;
use clap::{App, Arg, ArgMatches};
use inkwell::context::Context;
use inkwell::support::LLVMString;
use inkwell::targets::{
    CodeModel, FileType, InitializationConfig, RelocMode, Target, TargetMachine,
};
use inkwell::OptimizationLevel;

mod codegen;

fn parse_args() -> ArgMatches {
    App::new("Brainfucker")
        .version("1.0")
        .about("Brainfuck compliler")
        .arg(Arg::new("INPUT")
            .about("Brainfuck source file")
            .required(true)
            .index(1))
        .arg(Arg::new("compile").short('c')
            .long("compile")
            .about("Create object file only"))
        .arg(Arg::new("output")
            .short('o')
            .long("output")
            .takes_value(true)
            .value_name("FILE")
            .about("Sets output file"))
        .arg(Arg::new("optimize")
            .short('O')
            .long("optimize")
            .takes_value(true)
            .value_name("LEVEL")
            .about("Sets optimization level 0-3 (default 2)"))
        .arg(Arg::new("heap_size")
            .short('s')
            .long("heap-size")
            .takes_value(true)
            .value_name("BYTES")
            .about("Sets heap size in bytes (default 30000)"))
        .get_matches()
}

#[derive(Debug)]
struct LLVMError {
    error: String,
}

impl From<LLVMString> for LLVMError {
    fn from(error: LLVMString) -> Self {
        Self {
            error: error.to_string(),
        }
    }
}

impl From<String> for LLVMError {
    fn from(error: String) -> Self {
        Self { error }
    }
}

impl Display for LLVMError {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.error)
    }
}

impl Error for LLVMError {}

fn init_llvm(opt_level: OptimizationLevel) -> anyhow::Result<TargetMachine> {
    let config = InitializationConfig {
        asm_parser: false,
        asm_printer: true,
        base: true,
        disassembler: false,
        info: false,
        machine_code: true,
    };
    Target::initialize_native(&config).map_err(LLVMError::from)?;

    let triple = TargetMachine::get_default_triple();
    let target = Target::from_triple(&triple).map_err(LLVMError::from)?;

    let target_machine = target
        .create_target_machine(
            &triple,
            "generic",
            "",
            opt_level,
            RelocMode::Default,
            CodeModel::Default,
        )
        .context("cannot create target machine")?;

    Ok(target_machine)
}

fn main() -> anyhow::Result<()> {
    let args = parse_args();

    let opt_level = args
        .value_of("optimize")
        .unwrap_or("2")
        .parse::<u8>()
        .context("invalid optimization level")?;
    let opt_level = match opt_level {
        0 => OptimizationLevel::None,
        1 => OptimizationLevel::Less,
        2 => OptimizationLevel::Default,
        3 => OptimizationLevel::Aggressive,
        _ => anyhow::bail!("level must be in 0-3"),
    };

    let heap_size = args
        .value_of("heap_size")
        .unwrap_or("30000")
        .parse::<u64>()
        .context("invalid heap size value")?;

    let input = Path::new(args.value_of("INPUT").unwrap());
    let input_name = input.file_stem().context("invalid input path")?.to_str().unwrap();
    let input = fs::read_to_string(input).context("could not read input file")?;

    let target_machine = init_llvm(opt_level)?;
    let target_data = target_machine.get_target_data();
    let ctx = Context::create();

    let module = codegen::compile_module(&ctx, &target_data, &input_name, heap_size, &input);

    let obj_path = Path::new(&input_name).with_extension("o");
    target_machine
        .write_to_file(&module, FileType::Object, &obj_path)
        .map_err(LLVMError::from)?;

    if !args.is_present("compile") {
        let out_name = args.value_of("output").unwrap_or(input_name);

        let status = Command::new("ld")
            .args([
                obj_path.to_str().unwrap(),
                "/lib/crt1.o",
                "/lib/crti.o",
                "/lib/crtn.o",
                "-o",
                out_name,
                "-lc",
                "-dynamic-linker",
                "/lib64/ld-linux-x86-64.so.2",
            ])
            .status()
            .context("cannot start linker")?;
        fs::remove_file(&obj_path)?;

        if !status.success() {
            anyhow::bail!("linker exited with non-zero code");
        }
    }

    Ok(())
}
