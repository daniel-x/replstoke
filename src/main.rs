use std::ffi::OsString;

fn main() {
    let args: Vec<OsString> = std::env::args_os().skip(1).collect();
    std::process::exit(replstoke::run(&args));
}
