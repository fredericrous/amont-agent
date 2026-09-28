//! Print the generated scripts for a seed range, one `seed<TAB>script` per
//! line: `cargo run --example dump -- 1 500`.

fn main() {
    let mut args = std::env::args().skip(1).map(|a| a.parse::<u64>());
    let (Some(Ok(a)), Some(Ok(b))) = (args.next(), args.next()) else {
        eprintln!("usage: dump <first-seed> <last-seed>");
        std::process::exit(2);
    };
    for s in a..=b {
        println!("{s}\t{}", shell_oracle::gen::script(s));
    }
}
