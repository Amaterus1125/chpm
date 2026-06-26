// DO NOT add `mod ui;` here — ui lives inside the chiral lib crate.
// Importing from chiral:: gives us the single correct type.
use chiral::ui::ChiralUI;
use chiral::{install_binary, remove_binary, update_binary, search_packages, list_installed};
use std::env;

fn print_help() {
    println!("Chiral Package Manager v2.0");
    println!();
    println!("Usage:");
    println!("  chiral install <package>     Install a package");
    println!("  chiral remove  <package>     Remove an installed package");
    println!("  chiral update  <package>     Update a package to latest version");
    println!("  chiral upgrade               Update all installed packages");
    println!("  chiral search  <query>       Search available packages");
    println!("  chiral list                  List installed packages");
}

fn main() {
    let args: Vec<String> = env::args().collect();

    if args.len() < 2 {
        print_help();
        std::process::exit(1);
    }

    let mut ui = ChiralUI::new(false);

    let result = match args[1].as_str() {
        "install" => {
            if args.len() < 3 { eprintln!("Usage: chiral install <package>"); std::process::exit(1); }
            install_binary(&mut ui, &args[2])
        }
        "remove" => {
            if args.len() < 3 { eprintln!("Usage: chiral remove <package>"); std::process::exit(1); }
            remove_binary(&mut ui, &args[2])
        }
        "update" => {
            if args.len() < 3 { eprintln!("Usage: chiral update <package>"); std::process::exit(1); }
            update_binary(&mut ui, &args[2])
        }
        "upgrade" => {
            update_binary(&mut ui, "all")
        }
        "search" => {
            if args.len() < 3 { eprintln!("Usage: chiral search <query>"); std::process::exit(1); }
            search_packages(&args[2])
        }
        "list" => {
            list_installed()
        }
        _ => {
            eprintln!("Unknown command: {}", args[1]);
            print_help();
            std::process::exit(1);
        }
    };

    if let Err(e) = result {
        eprintln!("\n❌ Error: {}", e);
        std::process::exit(1);
    }
}
