//! CLI tool to display the PostgreSQL type compatibility matrix
//!
//! Usage:
//!   cargo run --bin type-matrix
//!   cargo run --bin type-matrix -- VARCHAR(100) VARCHAR(50)

use std::env;
use stonescriptdb_gateway::schema::{TypeChecker, TypeCompatibility};

fn main() {
    let args: Vec<String> = env::args().collect();

    let checker = TypeChecker::new();

    if args.len() == 3 {
        // Check specific type conversion
        let from_type = &args[1];
        let to_type = &args[2];

        println!("Checking: {} -> {}", from_type, to_type);
        println!();

        let result = checker.check_compatibility(from_type, to_type);

        match result {
            TypeCompatibility::Identical => {
                println!("Result: IDENTICAL");
                println!("  Types are the same, no change needed.");
            }
            TypeCompatibility::Safe => {
                println!("Result: SAFE");
                println!("  This type change can be performed without data loss.");
            }
            TypeCompatibility::DataLoss { reason } => {
                println!("Result: DATALOSS");
                println!("  This type change may cause data loss!");
                println!("  Reason: {}", reason);
                std::process::exit(1);
            }
            TypeCompatibility::Incompatible { reason } => {
                println!("Result: INCOMPATIBLE");
                println!("  These types cannot be converted!");
                println!("  Reason: {}", reason);
                std::process::exit(2);
            }
        }
    } else if args.len() == 1 {
        // Display full matrix
        print!("{}", checker.format_matrix());
    } else {
        eprintln!("Usage:");
        eprintln!("  {} <from_type> <to_type>  - Check specific conversion", args[0]);
        eprintln!("  {}                        - Display full matrix", args[0]);
        eprintln!();
        eprintln!("Examples:");
        eprintln!("  {} INTEGER BIGINT", args[0]);
        eprintln!("  {} \"VARCHAR(100)\" \"VARCHAR(50)\"", args[0]);
        eprintln!("  {} TIMESTAMP DATE", args[0]);
        std::process::exit(1);
    }
}
