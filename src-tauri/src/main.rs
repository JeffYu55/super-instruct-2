fn main() {
    if let Err(error) = super_instruct::run() {
        eprintln!("{error}");
        std::process::exit(1);
    }
}
