use std::io::{self, IsTerminal};

fn main() {
    let mut stdin = io::stdin().lock();
    let mut stdout = io::stdout().lock();
    let mut stderr = io::stderr().lock();
    let color = stderr.is_terminal() && std::env::var_os("NO_COLOR").is_none();
    let code = yaml_rt_cli::run_with_options(
        std::env::args_os(),
        &mut stdin,
        &mut stdout,
        &mut stderr,
        yaml_rt_cli::RunOptions { color },
    );
    if code != 0 {
        std::process::exit(code);
    }
}
