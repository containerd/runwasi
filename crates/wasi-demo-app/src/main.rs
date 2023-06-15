use std::{env, fs::File, io::prelude::*, process, thread::sleep, time::Duration};

fn main() {
    let args: Vec<_> = env::args().collect();
    let mut cmd = "daemon";
    if !args.is_empty() {
        cmd = &args[0];
    }

    match cmd {
        "echo" => println!("{}", &args[1..].join(" ")),
        "sleep" => sleep(Duration::from_secs_f64(args[1].parse::<f64>().unwrap())),
        "exit" => process::exit(args[1].parse::<i32>().unwrap()),
        "write" => {
            let mut file = File::create(&args[1]).unwrap();
            file.write_all(args[2..].join(" ").as_bytes()).unwrap();
        }
        "daemon" => loop {
            println!("This is a song that never ends.\nYes, it goes on and on my friends.\nSome people started singing it not knowing what it was,\nSo they'll continue singing it forever just because...\n");
            sleep(Duration::from_secs(1));
        },
        _ => {
            eprintln!("unknown command: {0}", args[0]);
            process::exit(1);
        }
    }

    eprintln!("exiting");
}
