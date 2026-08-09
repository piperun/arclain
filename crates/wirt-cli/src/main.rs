mod commands;

fn main() {
    if let Err(error) = commands::run(std::env::args_os().skip(1)) {
        eprintln!("wirt: {error:#}");
        std::process::exit(2);
    }
}
