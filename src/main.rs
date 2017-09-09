#[macro_use]
extern crate enum_primitive;

extern crate num;
extern crate serial;
extern crate clap;
extern crate bufstream;
extern crate time;

use std::io;
use std::io::{Error, ErrorKind};
// use std::io::{BufReader, BufWriter};
use std::time::Duration;
use std::process;
// use std::fs::File;
use std::path::Path;
use std::process::Command;

use num::FromPrimitive;
use std::io::prelude::*;
use serial::prelude::*;
use bufstream::BufStream;
use clap::{App, Arg};

#[derive(Debug, PartialEq)]
enum Board {
    Generic,
    St,
}

fn main() {
    let matches = App::new("gb-rw")
        .version("0.1")
        .about("Gameboy cartridge read/writer")
        .author("Dhole")
        .arg(Arg::with_name("baud")
            .help("Set the baud rate")
            .short("b")
            .long("baud")
            .value_name("RATE")
            //.default_value("115200")
            .default_value("1000000")
            .takes_value(true)
            .required(false)
            .validator(|baud| match baud.parse::<usize>() {
                Ok(_) => Ok(()),
                Err(e) => Err(format!("{}", e)),
            }))
        .arg(Arg::with_name("serial")
            .help("Set the serial device")
            .short("s")
            .long("serial")
            .value_name("DEVICE")
            .default_value("/dev/ttyACM0")
            .takes_value(true)
            .required(false))
        .arg(Arg::with_name("board")
            .help("Set the development board: generic, st")
            .short("d")
            .long("board")
            .value_name("BOARD")
            .default_value("st")
            .takes_value(true)
            .required(false)
            .validator(|board| match board.as_str() {
                "generic" => Ok(()),
                "st" => Ok(()),
                board => Err(format!("Invalid development board: {}", board)),
            }))
        .get_matches();

    let serial = matches.value_of("serial").unwrap();
    let baud = matches.value_of("baud").unwrap().parse::<usize>().unwrap();
    let board = match matches.value_of("board").unwrap() {
        "generic" => Board::Generic,
        "st" => Board::St,
        board => panic!("Invalid board: {}", board),
    };
    println!("Development board is: {:?}", board);
    println!("Using serial device: {} at baud rate: {}", serial, baud);

    let mut port_raw = match serial::open(serial) {
        Ok(port) => port,
        Err(e) => {
            println!("Error opening {}: {}", serial, e);
            process::exit(1);
        }
    };
    port_raw.configure(&serial::PortSettings {
            baud_rate: serial::BaudRate::from_speed(baud),
            char_size: serial::Bits8,
            parity: serial::ParityNone,
            stop_bits: serial::Stop1,
            flow_control: serial::FlowNone,
        })
        .unwrap_or_else(|e| {
            println!("Error configuring {}: {}", serial, e);
            process::exit(1);
        });
    port_raw.set_timeout(Duration::from_secs(3600 * 24)).unwrap_or_else(|e| {
        println!("Error setting timeout for {}: {}", serial, e);
        process::exit(1);
    });

    dev_reset(board).unwrap_or_else(|e| {
        println!("Error resetting development board: {}", e);
        process::exit(1);
    });

    let mut port = BufStream::new(port_raw);
    gb_rw(&mut port).unwrap();
}

fn dev_reset(board: Board) -> Result<(), io::Error> {
    match board {
        Board::Generic => {
            println!("Press the reset button on the board");
        }
        Board::St => {
            println!("\nResetting board using st-flash utility...");
            let output = Command::new("st-flash").arg("reset").output()?;
            println!("{}", String::from_utf8_lossy(&output.stderr));
            if !output.status.success() {
                return Err(Error::new(ErrorKind::Other,
                                      format!("st-flash returned with error code {:?}",
                                              output.status.code())));
            }
        }
    }
    Ok(())
}

enum_from_primitive! {
#[derive(Debug, PartialEq)]
enum Cmd {
    Read,
    Write,
    WriteRaw,
    WriteFlash,
    Erease,
}
}

fn gb_rw<T: SerialPort>(mut port: &mut BufStream<T>) -> Result<(), io::Error> {
    let mut buf = Vec::new();
    loop {
        try!(port.read_until(b'\n', &mut buf));
        if buf == b"HELLO\n" {
            break;
        }
        buf.clear();
    }
    println!("Connected!");

    // try!(port.write_all(b"s"));
    try!(port.write_all(vec![Cmd::Read as u8, 0x00, 0x00, 0x00, 0x01].as_slice()));
    try!(port.flush());

    let mut buf = vec![0; 16];
    let mut n = 0;
    loop {
        print!("{:06x}  ", n);
        port.read_exact(&mut buf)?;

        for i in 0..16 {
            print!("{:02x} ", buf[i]);
            if i == 7 {
                print!(" ");
            }
        }
        print!(" |");
        for b in &buf {
            let mut c = *b as char;
            if *b < 32 || *b > 126 {
                c = '.'
            }
            print!("{}", c);
        }
        print!("|\n");
        n += 16;
    }
}
