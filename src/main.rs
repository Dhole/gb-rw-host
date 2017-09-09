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
        .arg(
            Arg::with_name("baud")
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
            }),
        )
        .arg(
            Arg::with_name("serial")
                .help("Set the serial device")
                .short("s")
                .long("serial")
                .value_name("DEVICE")
                .default_value("/dev/ttyACM0")
                .takes_value(true)
                .required(false),
        )
        .arg(
            Arg::with_name("board")
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
                }),
        )
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
    port_raw
        .configure(&serial::PortSettings {
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

    port_clear(&mut port_raw).unwrap_or_else(|e| {
        println!("Error clearing port {}: {}", serial, e);
        process::exit(1);
    });

    port_raw
        .set_timeout(Duration::from_secs(3600 * 24))
        .unwrap_or_else(|e| {
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

/// port_clear will reset the port timeout!
fn port_clear<T: SerialPort>(mut port: &mut T) -> Result<(), io::Error> {
    port.set_timeout(Duration::from_millis(100))?;
    let mut buf = vec![0; 16];
    loop {
        match port.read(&mut buf) {
            Ok(0) => break,
            Ok(_) => continue,
            Err(e) => {
                if e.kind() == ErrorKind::TimedOut {
                    break;
                }
                return Err(e);
            }
        }
    }
    return Ok(());
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
                return Err(Error::new(
                    ErrorKind::Other,
                    format!(
                        "st-flash returned with error code {:?}",
                        output.status.code()
                    ),
                ));
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

enum_from_primitive! {
#[derive(Debug, PartialEq)]
enum ReplyCmd {
    Ack,
    Nack,
}
}

fn cmd_read(addr_start: u16, addr_end: u16) -> Vec<u8> {
    let addr_start_lo = (addr_start & 0xff) as u8;
    let addr_start_hi = ((addr_start & 0xff00) >> 8) as u8;
    let addr_end_lo = (addr_end & 0xff) as u8;
    let addr_end_hi = ((addr_end & 0xff00) >> 8) as u8;
    return vec![
        Cmd::Read as u8,
        addr_start_lo,
        addr_start_hi,
        addr_end_lo,
        addr_end_hi,
    ];
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

    let addr_start = 0x0000 as u16;
    let addr_end = 0x4000 as u16;
    port.write_all(cmd_read(addr_start, addr_end).as_slice())?;
    port.flush()?;

    //let mut ack = vec![0];
    //port.read_exact(&mut ack)?;
    //match ReplyCmd::from_u8(ack[0]) {
    //    Some(ReplyCmd::Ack) => {}
    //    Some(ReplyCmd::Nack) => {
    //        return Err(Error::new(ErrorKind::Other, "Board replied NACK"));
    //    }
    //    None => {
    //        return Err(Error::new(ErrorKind::Other, "Board reply is invalid"));
    //    }
    //}

    let mut buf = vec![0; (addr_end - addr_start) as usize];
    //let mut buf = vec![0; 0x600];
    port.read_exact(&mut buf)?;
    print_hex(&buf, 0x0000);
    println!();

    return Ok(());
}

fn print_hex(buf: &[u8], addr_start: u16) {
    //for n in (0..100).step_by(16) { // Unstable :(
    for n in (0..div_round_up(buf.len(), 16)).map(|n| n * 16) {
        print!("{:06x}  ", (addr_start as usize) + n);
        for i in 0..16 {
            if n + i < buf.len() {
                print!("{:02x} ", buf[n + i])
            } else {
                print!("   ")
            }
            if i == 7 {
                print!(" ");
            }
        }
        print!(" |");
        for i in 0..16 {
            let c = if n + i < buf.len() {
                let b = buf[n + i];
                if b < 32 || b > 126 { '.' } else { b as char }
            } else {
                ' '
            };
            print!("{}", c);
        }
        print!("|\n");
    }
}

fn div_round_up(x: usize, y: usize) -> usize {
    return (x + y - 1) / y;
}
