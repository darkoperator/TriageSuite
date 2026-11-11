fn main() {
    let code = oxl_cli::run(std::env::args_os(), &mut std::io::stdout());
    std::process::exit(code);
}
