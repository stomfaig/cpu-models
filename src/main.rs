mod alu;
mod five_stage;
mod memory;
mod simple_ooo;
mod utils;

use five_stage::{FiveStageCpu, forwarding, hazard};
use std::env;
use utils::diag::Diagnosable as _;

fn usage() -> ! {
    eprintln!("Usage: cpu-simulator <model> <program>");
    eprintln!();
    eprintln!("Models:");
    eprintln!("  five-stage    5-stage in-order pipeline (load-use stall + full forwarding)");
    eprintln!("  ooo           Simple out-of-order (Tomasulo-style)");
    eprintln!();
    eprintln!("Program: inline assembly string, e.g. \"addi x1, x0, 42\"");
    std::process::exit(1);
}

fn main() {
    let args: Vec<String> = env::args().collect();
    if args.len() < 3 {
        usage();
    }

    let model = &args[1];
    let program = &args[2];

    // Accept either a file path or an inline assembly string.
    let source = if std::path::Path::new(program).exists() {
        std::fs::read_to_string(program).unwrap_or_else(|e| {
            eprintln!("Could not read file: {e}");
            std::process::exit(1);
        })
    } else {
        program.clone()
    };

    let words = utils::assembler::assemble(&source).unwrap_or_else(|e| {
        eprintln!("Assembly error: {e}");
        std::process::exit(1);
    });

    match model.as_str() {
        "five-stage" => run_five_stage(words),
        "ooo" => run_ooo(words),
        _ => {
            eprintln!("Unknown model: {model}");
            usage();
        }
    }
}

fn run_five_stage(words: Vec<u32>) {
    let hazard: Box<dyn hazard::HazardPolicy> = Box::new(hazard::StallOnLoad);

    let mut cpu = FiveStageCpu::new(1024, hazard, Box::new(forwarding::FullForwarding));
    for (i, &w) in words.iter().enumerate() {
        cpu.load(i * 4, &w.to_le_bytes());
    }

    let cycles = words.len() + 4; // drain the pipeline
    for _ in 0..cycles {
        cpu.tick();
        cpu.diagnose().print();
        print!("cycle {:3} |", cpu.cycle);
        for (i, &v) in cpu.regs.iter().enumerate().skip(1) {
            if v != 0 {
                print!(" x{i}={v}");
            }
        }
        println!();
    }
}

fn run_ooo(words: Vec<u32>) {
    let mut cpu = simple_ooo::build_default();
    // load program into memory
    for (i, &w) in words.iter().enumerate() {
        cpu.load(i * 4, &w.to_le_bytes());
    }

    // sample prog runs for around 52 cycles.
    // TODO: detect program halting automatically
    let cycles = 52;
    for c in 0..cycles {
        cpu.tick();
        cpu.diagnose().print();
        print!("cycle {:3} |", c + 1);
        for (i, &v) in cpu.regs.iter().enumerate().skip(1) {
            if v != 0 {
                print!(" x{i}={v}");
            }
        }
        println!();
    }
}
