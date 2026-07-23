use std::io;

fn main() {
    let mut stdin = io::stdin().lock();
    let mut stdout = io::stdout().lock();
    let mut stderr = io::stderr().lock();
    let code = yaml_rt_cli::run(std::env::args_os(), &mut stdin, &mut stdout, &mut stderr);
    if code != 0 {
        std::process::exit(code);
    }
}
