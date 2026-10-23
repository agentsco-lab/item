//! item-secret: a secret into the keyring, or out of it (secrets.rs).
//!
//!   item-secret set NAME   (the value on standard input, one line)
//!   item-secret get NAME
//!   item-secret list       (the names kept, never the values)
//!   item-secret forget NAME
//!
//! The shell's keys for the services it speaks to go in this way - the
//! transcription's, `item-secret set openrouter` - from a terminal on the
//! phone or over ssh (`echo KEY | itemgrid run --user 'item-secret set
//! openrouter'`), never into a file.

#[path = "../bus.rs"]
#[allow(dead_code)]
mod bus;
#[path = "../secrets.rs"]
mod secrets;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let result = match (args.get(1).map(String::as_str), args.get(2)) {
        (Some("set"), Some(name)) => {
            let mut value = String::new();
            let _ = std::io::Read::read_to_string(&mut std::io::stdin(), &mut value);
            let value = value.trim();
            if value.is_empty() {
                Err("nothing to keep: the value goes on standard input".to_owned())
            } else {
                secrets::set(name, value).map(|()| format!("{name}: kept"))
            }
        }
        (Some("get"), Some(name)) => secrets::get(name).and_then(|v| v.ok_or_else(|| format!("{name}: nothing kept"))),
        (Some("list"), _) => secrets::list().map(|names| names.join("\n")),
        (Some("forget"), Some(name)) => secrets::forget(name).map(|was| if was { format!("{name}: forgotten") } else { format!("{name}: nothing kept") }),
        _ => Err("item-secret set NAME | get NAME | list | forget NAME".to_owned()),
    };
    match result {
        Ok(out) => println!("{out}"),
        Err(e) => {
            eprintln!("item-secret: {e}");
            std::process::exit(1);
        }
    }
}
