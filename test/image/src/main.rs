use std::{env, fs::File, io::prelude::*, process, thread::sleep, time::Duration};

fn main() {
    let args: Vec<_> = env::args().collect();
    let mut cmd = "daemon";
    if args.len() >= 2 {
        cmd = &args[1];
    }

    match cmd {
        "echo" => println!("{}", &args[2..].join(" ")),
        "sleep" => sleep(Duration::from_secs_f64(args[2].parse::<f64>().unwrap())),
        "exit" => process::exit(args[2].parse::<i32>().unwrap()),
        "write" => {
            let mut file = File::create(&args[2]).unwrap();
            file.write_all(&args[3..].join(" ").as_bytes()).unwrap();
        }
        "daemon" => loop {
            println!("This is a song that never ends.\nYes, it goes on and on my friends.\nSome people started singing it not knowing what it does,\nSo they'll continue singing it forever just because...\n");
            sleep(Duration::from_secs(1));
        },
        _ => {
            eprintln!("unknown command: {0}", args[1]);
            process::exit(1);
        }
    }

    eprintln!("exiting");
}
