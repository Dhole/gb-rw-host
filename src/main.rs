#[macro_use]
extern crate enum_primitive;
extern crate num;
extern crate serial;
extern crate clap;
extern crate bufstream;
extern crate time;

extern crate gb_rw_host;

use std::io;
use std::io::{Error, ErrorKind};
// use std::io::{BufReader, BufWriter};
use std::time::Duration;
use std::process;
use std::fs;
use std::fs::OpenOptions;
use std::fs::File;
//use std::path::Path;
use std::process::Command;

use std::io::prelude::*;
use serial::prelude::*;
use bufstream::BufStream;
use clap::{App, Arg};

use gb_rw_host::header::*;
use gb_rw_host::utils::*;

#[derive(Debug, PartialEq)]
enum Board {
    Generic,
    St,
}

#[derive(Debug, PartialEq)]
enum Mode {
    ReadROM,
    ReadRAM,
    WriteROM,
    WriteRAM,
    Erase,
    Read,
    Test,
}

#[derive(Debug, PartialEq)]
enum Memory {
    Rom,
    Ram,
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
        .arg(
            Arg::with_name("mode")
                .help(
                    "Set the operation mode: read_ROM, read_RAM, write_ROM, write_RAM",
                )
                .short("m")
                .long("mode")
                .value_name("MODE")
                .takes_value(true)
                .required(true)
                .validator(|mode| match mode.as_str() {
                    "read_ROM" => Ok(()),
                    "read_RAM" => Ok(()),
                    "write_ROM" => Ok(()),
                    "write_RAM" => Ok(()),
                    "erase" => Ok(()),
                    "read" => Ok(()),
                    "test" => Ok(()),
                    mode => Err(format!("Invalid operation mode: {}", mode)),
                }),
        )
        .arg(
            Arg::with_name("file")
                .help("Set the file to read/write for the cartridge ROM/RAM")
                .short("f")
                .long("file")
                .value_name("FILE")
                .takes_value(true)
                .required(true),
        )
        .get_matches();

    let serial = matches.value_of("serial").unwrap();
    let baud = matches.value_of("baud").unwrap().parse::<usize>().unwrap();
    let board = match matches.value_of("board").unwrap() {
        "generic" => Board::Generic,
        "st" => Board::St,
        board => panic!("Invalid board: {}", board),
    };
    let mode = match matches.value_of("mode").unwrap() {
        "read_ROM" => Mode::ReadROM,
        "read_RAM" => Mode::ReadRAM,
        "write_ROM" => Mode::WriteROM,
        "write_RAM" => Mode::WriteRAM,
        "erase" => Mode::Erase,
        "read" => Mode::Read,
        "test" => Mode::Test,
        mode => panic!("Invalid operation mode: {}", mode),
    };
    let path = matches.value_of("file").unwrap();

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
    gb_rw(&mut port, mode, path).unwrap_or_else(|e| {
        println!("Error during operation: {}", e);
        process::exit(1);
    });

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
    Reset,
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

fn cmd_write(addr: u16, data: u8) -> Vec<u8> {
    let addr_start_lo = (addr & 0xff) as u8;
    let addr_start_hi = ((addr & 0xff00) >> 8) as u8;
    return vec![Cmd::Write as u8, addr_start_lo, addr_start_hi, data, 0x00];
}

fn gb_rw<T: SerialPort>(
    mut port: &mut BufStream<T>,
    mode: Mode,
    path: &str,
) -> Result<(), io::Error> {
    let mut buf = Vec::new();
    loop {
        try!(port.read_until(b'\n', &mut buf));
        if buf == b"HELLO\n" {
            break;
        }
        buf.clear();
    }
    println!("Connected!");

    let file = match mode {
        Mode::ReadROM => {
            println!("Reading cartridge ROM into {}", path);
            OpenOptions::new().write(true).create_new(true).open(path)?
        }
        Mode::ReadRAM => {
            println!("Reading cartridge RAM into {}", path);
            OpenOptions::new().write(true).create_new(true).open(path)?
        }
        Mode::WriteROM => {
            println!("Writing {} into cartridge ROM", path);
            OpenOptions::new().read(true).open(path)?
        }
        Mode::WriteRAM => {
            println!("Writing {} into cartridge RAM", path);
            OpenOptions::new().read(true).open(path)?
        }
        Mode::Erase => OpenOptions::new().read(true).open("/dev/null")?,
        Mode::Read => OpenOptions::new().read(true).open("/dev/null")?,
        Mode::Test => OpenOptions::new().read(true).open("/dev/null")?,
    };
    println!();

    // Not sure if this is useful
    port.write_all(
        vec![Cmd::Reset as u8, 0x00, 0x00, 0x00, 0x00]
            .as_slice(),
    )?;
    port.flush()?;

    let result = match mode {
        Mode::ReadROM => read(&mut port, &file, Memory::Rom),
        Mode::Erase => erase(&mut port),
        Mode::Read => read(&mut port),
        Mode::Test => test(&mut port),
        ref m => Err(Error::new(
            ErrorKind::Other,
            format!("Error: operation mode {:?} not implemented yet", m),
        )),
    };

    return if mode == Mode::ReadROM || mode == Mode::ReadRAM {
        result.map_err(|e| {
            drop(file);
            fs::remove_file(path).unwrap_or(());
            return e;
        })
    } else {
        result
    };

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
}

fn read<T: SerialPort>(
    mut port: &mut BufStream<T>,
    mut file: &File,
    memory: Memory,
) -> Result<(), io::Error> {

    // Read Bank 00
    println!("Reading bank 000");
    let addr_start = 0x0000 as u16;
    let addr_end = 0x4000 as u16;
    port.write_all(cmd_read(addr_start, addr_end).as_slice())?;
    port.flush()?;

    let mut buf = vec![0; (addr_end - addr_start) as usize];
    port.read_exact(&mut buf)?;

    println!();
    //print_hex(&buf[0x0000..0x0200], 0x0000);
    //println!();

    let header_info = match parse_header(&buf) {
        Ok(header_info) => header_info,
        Err(e) => {
            return Err(Error::new(
                ErrorKind::Other,
                format!("Error parsing cartridge header: {:?}", e),
            ));
        }
    };

    let header_checksum = header_checksum(&buf);

    print_header(&header_info);
    println!();

    if header_info.checksum != header_checksum {
        return Err(Error::new(
            ErrorKind::Other,
            format!(
                "Header checksum mismatch: {:02x} != {:02x}",
                header_info.checksum,
                header_checksum
            ),
        ));
    }

    let mut mem = Vec::new();
    match memory {
        Memory::Rom => {
            mem.extend_from_slice(&buf);
            let addr_start = 0x4000 as u16;
            let addr_end = 0x8000 as u16;
            for bank in 1..(header_info.rom_banks + 1) {
                // Pipeline requests and reads
                if bank != header_info.rom_banks {
                    println!("Switching to bank {:03}", bank);
                    bank_switch(&mut port, &header_info.mem_controller, &memory, bank)?;
                    println!("Reading bank {:03}", bank);
                    port.write_all(cmd_read(addr_start, addr_end).as_slice())?;
                    port.flush()?;
                }
                if bank != 1 {
                    let mut buf = vec![0; (addr_end - addr_start) as usize];
                    port.read_exact(&mut buf)?;
                    mem.extend_from_slice(&buf);
                }
            }
            println!();
            let global_checksum = global_checksum(&mem);
            if header_info.global_checksum != global_checksum {
                println!(
                    "Global checksum mismatch: {:02x} != {:02x}",
                    header_info.global_checksum,
                    global_checksum
                );
            //return Err(Error::new(
            //    ErrorKind::Other,
            //    format!(
            //        "Global checksum mismatch: {:02x} != {:02x}",
            //        header_info.global_checksum,
            //        global_checksum
            //    ),
            //));
            } else {
                println!("Global checksum verification successfull!");
            }
        }
        Memory::Ram => (),
    }
    println!("Writing file...");
    file.write_all(&mem)?;

    return Ok(());
}

fn bank_switch<T: SerialPort>(
    mut port: &mut BufStream<T>,
    mem_controller: &MemController,
    memory: &Memory,
    bank: usize,
) -> Result<(), io::Error> {
    let mut writes: Vec<(u16, u8)> = Vec::new();
    match *mem_controller {
        MemController::None => {
            if bank != 1 {
                return Err(Error::new(
                    ErrorKind::Other,
                    "ROM Only cartridges can't select bank",
                ));
            }
        }
        MemController::Mbc1 => {
            match *memory {
                Memory::Rom => {
                    writes.push((0x6000, 0x00)); // Select ROM Banking Mode for upper two bits
                    writes.push((0x2000, (bank as u8) & 0x1f)); // Set bank bits 0..4
                    writes.push((0x4000, ((bank as u8) & 0x60) >> 5)); // Set bank bits 5,6
                }
                ref mem => {
                    return Err(Error::new(
                        ErrorKind::Other,
                        format!("Error: Memory {:?} not implemented yet", mem),
                    ))
                }
            }
        }
        MemController::Mbc5 => {
            match *memory {
                Memory::Rom => {
                    writes.push((0x2000, ((bank as u16) & 0x00ff) as u8)); // Set bank bits 0..7
                    writes.push((0x3000, (((bank as u16) & 0x0100) >> 8) as u8)); // Set bank bit 8
                }
                ref mem => {
                    return Err(Error::new(
                        ErrorKind::Other,
                        format!("Error: Memory {:?} not implemented yet", mem),
                    ))
                }
            }
        }
        ref mc => {
            return Err(Error::new(
                ErrorKind::Other,
                format!(
                    "Error: Memory controller {:?} not implemented yet",
                    mc
                ),
            ))
        }
    }
    for (addr, data) in writes {
        port.write_all(cmd_write(addr, data).as_slice())?;
    }
    port.flush()?;
    return Ok(());
}

fn erase<T: SerialPort>(mut port: &mut BufStream<T>) -> Result<(), io::Error> {

    //let addr_start = 0x0000 as u16;
    //let addr_end = 0x4000 as u16;
    //port.write_all(cmd_read(addr_start, addr_end).as_slice())?;
    //port.flush()?;

    //let mut buf = vec![0; (addr_end - addr_start) as usize];
    //port.read_exact(&mut buf)?;

    //print_hex(&buf[0x0000..0x0400], 0x0000);
    //println!();

    bank_switch(&mut port, &MemController::Mbc5, &Memory::Rom, 1)?;

    let writes = vec![
        //(0x0AAA, 0xAA),
        //(0x0555, 0x55),
        //(0x0AAA, 0x80),
        //(0x0AAA, 0xAA),
        //(0x0555, 0x55),
        //(0x0000, 0x30),
        //
        //(0x0A00, 0x07),
        //(0x3f00, 0x41),
        //(0x5555, 0xAA),
        //(0x2AAA, 0x55),
        //(0x5555, 0x80),
        //(0x5555, 0xAA),
        //(0x2AAA, 0x55),
        //(0x5555, 0x10),
        //
        (0x0AAA, 0xA9),
        (0x0555, 0x56),
        (0x0AAA, 0x80),
        (0x0AAA, 0xA9),
        (0x0555, 0x56),
        (0x0AAA, 0x10),
    ];

    for (addr, data) in writes {
        port.write_all(cmd_write(addr, data).as_slice())?;
    }
    port.flush()?;

    // TODO: Wait until data has been ereased (read addr = 0x0000 must be 0xff)

    //port.write_all(cmd_read(addr_start, addr_end).as_slice())?;
    //port.flush()?;

    //let mut buf = vec![0; (addr_end - addr_start) as usize];
    //port.read_exact(&mut buf)?;

    //print_hex(&buf[0x0000..0x0200], 0x0000);
    //println!();

    return Ok(());
}
