fn main() {
    let output = getter_cli::run_with_command_sink(std::env::args(), &mut std::io::stderr());
    if !output.stdout.is_empty() {
        print!("{}", output.stdout);
    }
    if !output.stderr.is_empty() {
        eprint!("{}", output.stderr);
    }
    std::process::exit(output.exit_code.code());
}
