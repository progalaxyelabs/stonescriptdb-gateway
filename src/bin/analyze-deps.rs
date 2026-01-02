//! CLI tool to analyze table dependencies in a schema folder
//!
//! Usage:
//!   cargo run --bin analyze-deps -- /path/to/postgresql/tables
//!   cargo run --bin analyze-deps -- /path/to/postgresql/migrations

use std::env;
use std::path::Path;

// Import from the main crate
use stonescriptdb_gateway::schema::DependencyAnalyzer;

fn main() {
    let args: Vec<String> = env::args().collect();

    if args.len() < 2 {
        eprintln!("Usage: {} <path-to-tables-or-migrations-folder>", args[0]);
        eprintln!("");
        eprintln!("Examples:");
        eprintln!("  {} ./postgresql/tables", args[0]);
        eprintln!("  {} ./postgresql/migrations", args[0]);
        std::process::exit(1);
    }

    let path = Path::new(&args[1]);

    if !path.exists() {
        eprintln!("Error: Path does not exist: {}", path.display());
        std::process::exit(1);
    }

    if !path.is_dir() {
        eprintln!("Error: Path is not a directory: {}", path.display());
        std::process::exit(1);
    }

    println!("Analyzing table dependencies in: {}", path.display());
    println!("");

    match DependencyAnalyzer::analyze_directory(path) {
        Ok(analysis) => {
            print!("{}", DependencyAnalyzer::format_analysis(&analysis));

            // Exit with error code if there are circular dependencies
            if !analysis.circular_dependencies.is_empty() {
                std::process::exit(2);
            }
        }
        Err(e) => {
            eprintln!("Error analyzing dependencies: {}", e);
            std::process::exit(1);
        }
    }
}
