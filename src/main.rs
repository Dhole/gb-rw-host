#[macro_use]
extern crate enum_primitive;
extern crate num;
extern crate serial;
extern crate clap;
extern crate bufstream;
extern crate time;
extern crate zip;

extern crate gb_rw_host;

use std::io;
use std::io::{Error, ErrorKind};
// use std::io::{BufReader, BufWriter};
use std::time::Duration;
use std::process;
use std::fs;
use std::fs::OpenOptions;
use std::fs::File;
use std::path::Path;
use std::process::Command;
use std::thread;
use std::ffi::OsStr;

use std::io::prelude::*;
use serial::prelude::*;
use bufstream::BufStream;
use clap::{App, Arg};
use zip::read::ZipArchive;

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
    let path = Path::new(matches.value_of("file").unwrap());

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
        .set_timeout(Duration::from_secs(8))
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
    Ping
}
}

enum_from_primitive! {
#[derive(Debug, PartialEq)]
enum CmdReply {
    DMAReady,
    DMANotReady,
    Pong
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

fn cmd_write_flash(addr_start: u16, addr_end: u16) -> Vec<u8> {
    let addr_start_lo = (addr_start & 0xff) as u8;
    let addr_start_hi = ((addr_start & 0xff00) >> 8) as u8;
    let addr_end_lo = (addr_end & 0xff) as u8;
    let addr_end_hi = ((addr_end & 0xff00) >> 8) as u8;
    return vec![
        Cmd::WriteFlash as u8,
        addr_start_lo,
        addr_start_hi,
        addr_end_lo,
        addr_end_hi,
    ];
}

fn cmd_write_raw(addr_start: u16, addr_end: u16) -> Vec<u8> {
    let addr_start_lo = (addr_start & 0xff) as u8;
    let addr_start_hi = ((addr_start & 0xff00) >> 8) as u8;
    let addr_end_lo = (addr_end & 0xff) as u8;
    let addr_end_hi = ((addr_end & 0xff00) >> 8) as u8;
    return vec![
        Cmd::WriteRaw as u8,
        addr_start_lo,
        addr_start_hi,
        addr_end_lo,
        addr_end_hi,
    ];
}

fn cmd_ping() -> Vec<u8> {
    return vec![Cmd::Ping as u8, 0x00, 0x00, 0x00, 0x00];
}

fn gb_rw<T: SerialPort>(
    mut port: &mut BufStream<T>,
    mode: Mode,
    path: &Path,
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

    // Not sure if this is useful
    //port.write_all(
    //    vec![Cmd::Reset as u8, 0x00, 0x00, 0x00, 0x00]
    //        .as_slice(),
    //)?;
    //port.flush()?;

    let result = match mode {
        Mode::ReadROM => {
            println!("Reading cartridge ROM into {}", path.display());
            let file = OpenOptions::new().write(true).create_new(true).open(path)?;
            read(&mut port, &file, Memory::Rom)
        }
        Mode::ReadRAM => {
            println!("Reading cartridge RAM into {}", path.display());
            let file = OpenOptions::new().write(true).create_new(true).open(path)?;
            read(&mut port, &file, Memory::Ram)
        }
        Mode::WriteROM => {
            println!("Writing {} into cartridge ROM", path.display());
            let mut rom = Vec::new();
            if path.extension() == Some(OsStr::new("zip")) {
                let mut zip = ZipArchive::new(File::open(path)?)?;
                for i in 0..zip.len() {
                    let mut zip_file = zip.by_index(i).unwrap();
                    let filename = zip_file.name().to_string();
                    let extension = Path::new(&filename).extension();
                    if extension == Some(OsStr::new("gb")) || extension == Some(OsStr::new("gbc")) {
                        zip_file.read_to_end(&mut rom)?;;
                        break;
                    }
                }
                if rom.len() == 0 {
                    return Err(Error::new(
                        ErrorKind::Other,
                        format!("File {} doesn't contain any ROM", path.display()),
                    ));
                }
            } else {
                OpenOptions::new().read(true).open(path)?.read_to_end(
                    &mut rom,
                )?;
            }
            write_flash(&mut port, &rom)
        }
        Mode::WriteRAM => {
            println!("Writing {} into cartridge RAM", path.display());
            let mut rom = Vec::new();
            OpenOptions::new().read(true).open(path)?.read_to_end(
                &mut rom,
            )?;
            write_ram(&mut port, &rom)
        }
        Mode::Erase => erase(&mut port),
        Mode::Read => read_test(&mut port),
        Mode::Test => test3(&mut port),
    };
    println!();

    return if mode == Mode::ReadROM || mode == Mode::ReadRAM {
        result.map_err(|e| {
            //drop(file);
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
    println!("Reading ROM bank 000");
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
                    println!("Switching to ROM bank {:03}", bank);
                    switch_bank(&mut port, &header_info.mem_controller, &memory, bank)?;
                    println!("Reading ROM bank {:03}", bank);
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
            } else {
                println!("Global checksum verification successfull!");
            }
        }
        Memory::Ram => {
            ram_enable(&mut port)?;
            let addr_start = 0xA000 as u16;
            let addr_end = if header_info.ram_size < 0x2000 {
                0xA000 + header_info.ram_size as u16
            } else {
                0xC000 as u16
            };
            for bank in 0..(header_info.ram_banks + 1) {
                // Pipeline requests and reads
                if bank != header_info.ram_banks {
                    println!("Switching to RAM bank {:03}", bank);
                    switch_bank(&mut port, &header_info.mem_controller, &memory, bank)?;
                    println!("Reading RAM bank {:03}", bank);
                    port.write_all(cmd_read(addr_start, addr_end).as_slice())?;
                    port.flush()?;
                }
                if bank != 0 {
                    let mut buf = vec![0; (addr_end - addr_start) as usize];
                    port.read_exact(&mut buf)?;
                    mem.extend_from_slice(&buf);
                }
            }
            println!();
            ram_disable(&mut port)?;
        }
    }
    println!("Writing file...");
    return file.write_all(&mem);
}

fn switch_bank<T: SerialPort>(
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
                Memory::Ram => {
                    writes.push((0x6000, 0x01)); // Select RAM Banking Mode
                    writes.push((0x4000, (bank as u8) & 0x03)); // Set bank bits 0..4
                }
            }
        }
        MemController::Mbc5 => {
            match *memory {
                Memory::Rom => {
                    writes.push((0x2000, ((bank as u16) & 0x00ff) as u8)); // Set bank bits 0..7
                    writes.push((0x3000, (((bank as u16) & 0x0100) >> 8) as u8)); // Set bank bit 8
                }
                Memory::Ram => {
                    writes.push((0x4000, (bank as u8) & 0x0f)); // Set bank bits 0..4
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

fn ram_enable<T: SerialPort>(mut port: &mut BufStream<T>) -> Result<(), io::Error> {
    port.write_all(cmd_write(0x0000, 0x0A).as_slice())?;
    return port.flush();
}

fn ram_disable<T: SerialPort>(mut port: &mut BufStream<T>) -> Result<(), io::Error> {
    port.write_all(cmd_write(0x0000, 0x00).as_slice())?;
    return port.flush();
}

fn erase<T: SerialPort>(mut port: &mut BufStream<T>) -> Result<(), io::Error> {
    return erase_flash(&mut port);
}

fn erase_flash<T: SerialPort>(mut port: &mut BufStream<T>) -> Result<(), io::Error> {

    switch_bank(&mut port, &MemController::Mbc5, &Memory::Rom, 1)?;

    let writes = vec![
        (0x0AAA, 0xA9),
        (0x0555, 0x56),
        (0x0AAA, 0x80),
        (0x0AAA, 0xA9),
        (0x0555, 0x56),
        (0x0AAA, 0x10), // All
        //(0x0000, 0x30), // First segment
    ];

    println!("Erasing flash, wait...");
    for (addr, data) in writes {
        port.write_all(cmd_write(addr, data).as_slice())?;
    }
    port.flush()?;

    loop {
        thread::sleep(Duration::from_millis(500));
        port.write_all(cmd_read(0x0000, 0x0001).as_slice())?;
        port.flush()?;
        let mut buf = vec![0; 1];
        port.read_exact(&mut buf)?;
        if buf[0] == 0xFF {
            break;
        }
    }
    println!("OK!");

    return Ok(());
}

fn read_test<T: SerialPort>(mut port: &mut BufStream<T>) -> Result<(), io::Error> {

    //let addr_start = 0x0000 as u16;
    //let addr_end = 0x4000 as u16;
    //port.write_all(cmd_read(addr_start, addr_end).as_slice())?;
    //port.flush()?;

    //let mut buf = vec![0; (addr_end - addr_start) as usize];
    //port.read_exact(&mut buf)?;

    //print_hex(&buf[0x0000..0x0400], 0x0000);
    //println!();

    switch_bank(&mut port, &MemController::Mbc5, &Memory::Rom, 1)?;

    let addr_start = 0x0000 as u16;
    let addr_end = 0x4000 as u16;
    port.write_all(cmd_read(addr_start, addr_end).as_slice())?;
    port.flush()?;

    let mut buf = vec![0; (addr_end - addr_start) as usize];
    port.read_exact(&mut buf)?;

    print_hex(&buf[0x0000..0x0200], 0x0000);
    println!();

    let addr_start = 0x4000 as u16;
    let addr_end = 0x8000 as u16;
    port.write_all(cmd_read(addr_start, addr_end).as_slice())?;
    port.flush()?;

    let mut buf = vec![0; (addr_end - addr_start) as usize];
    port.read_exact(&mut buf)?;

    print_hex(&buf[0x0000..0x0200], 0x4000);
    println!();
    return Ok(());
}

fn test1<T: SerialPort>(mut port: &mut BufStream<T>) -> Result<(), io::Error> {

    let mut rom = Vec::new();
    //let mut file = OpenOptions::new().read(true).open("tetris.gb")?;
    let mut file = OpenOptions::new().read(true).open("/tmp/fbgb_v2/fbgb.gb")?;
    file.read_to_end(&mut rom)?;

    for bank in 0..2 {
        if bank != 0 {
            switch_bank(&mut port, &MemController::Mbc5, &Memory::Rom, bank)?;
        }
        let addr_start = if bank == 0 { 0x0000 } else { 0x4000 };
        let writes = vec![
            (0x0AAA, 0xA9),
            (0x0555, 0x56),
            (0x0AAA, 0xA0),
            (addr_start, 0xFF),
        ];
        for (addr, data) in writes {
            port.write_all(cmd_write(addr, data).as_slice())?;
        }
        port.flush()?;
        for addr in 0..0x4000 {
            let writes = vec![
                (0x0AAA, 0xA9),
                (0x0555, 0x56),
                (0x0AAA, 0xA0),
                (addr_start + addr as u16, rom[bank * 0x4000 + addr]),
            ];
            for (addr, data) in writes {
                port.write_all(cmd_write(addr, data).as_slice())?;
            }
            port.flush()?;
            println!("0x{:04x}", (bank * 0x4000 + addr));
        }
        thread::sleep(Duration::from_millis(4000));
    }
    //for (addr, b) in rom.iter().enumerate() {
    //    if ((addr as u16) % 0x2000) == 0 {
    //        let writes = vec![
    //            (0x0AAA, 0xA9),
    //            (0x0555, 0x56),
    //            (0x0AAA, 0xA0),
    //            (addr as u16, 0xFF),
    //        ];
    //        for (addr, data) in writes {
    //            port.write_all(cmd_write(addr, data).as_slice())?;
    //        }
    //        port.flush()?;
    //        println!("0x{:04x}", addr as u16);
    //    }
    //    let writes = vec![
    //        (0x0AAA, 0xA9),
    //        (0x0555, 0x56),
    //        (0x0AAA, 0xA0),
    //        (addr as u16, *b),
    //    ];
    //    for (addr, data) in writes {
    //        port.write_all(cmd_write(addr, data).as_slice())?;
    //    }
    //    port.flush()?;
    //    //println!("{}%", (addr as f64) / (rom.len() as f64));
    //    //println!("0x{:04x}", addr as u16);
    //}

    //let writes = vec![
    //    (0x0AAA, 0xA9),
    //    (0x0555, 0x56),
    //    (0x0AAA, 0xA0),
    //    (0x4000, 0xFF),
    //    (0x0AAA, 0xA9),
    //    (0x0555, 0x56),
    //    (0x0AAA, 0xA0),
    //    (0x4000, 0x41),
    //];
    //for (addr, data) in writes {
    //    port.write_all(cmd_write(addr, data).as_slice())?;
    //}
    //port.flush()?;

    //let addr_start = 0x0000 as u16;
    //let addr_end = 0x4000 as u16;
    //port.write_all(cmd_read(addr_start, addr_end).as_slice())?;
    //port.flush()?;

    //let mut buf = vec![0; (addr_end - addr_start) as usize];
    //port.read_exact(&mut buf)?;

    //print_hex(&buf[0x0000..0x0200], 0x0000);
    //println!();
    return Ok(());
}

fn test2<T: SerialPort>(mut port: &mut BufStream<T>) -> Result<(), io::Error> {

    switch_bank(&mut port, &MemController::Mbc5, &Memory::Rom, 1)?;
    let writes = vec![
        (0x0AAA, 0xA9),
        (0x0555, 0x56),
        (0x0AAA, 0xA0),
        (0x0001, 0x42),
    ];
    for (addr, data) in writes {
        port.write_all(cmd_write(addr, data).as_slice())?;
    }
    port.flush()?;

    return Ok(());
}

fn write_ram<T: SerialPort>(mut port: &mut BufStream<T>, sav: &[u8]) -> Result<(), io::Error> {

    // Read Bank 00
    println!("Reading ROM bank 000");
    let addr_start = 0x0000 as u16;
    let addr_end = 0x4000 as u16;
    port.write_all(cmd_read(addr_start, addr_end).as_slice())?;
    port.flush()?;

    let mut buf = vec![0; (addr_end - addr_start) as usize];
    port.read_exact(&mut buf)?;

    println!();

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

    if sav.len() != header_info.ram_size {
        return Err(Error::new(
            ErrorKind::Other,
            format!(
                "RAM size mismatch between header and file: {:02x} != {:02x}",
                header_info.ram_size,
                sav.len()
            ),
        ));
    }

    ram_enable(&mut port)?;
    let addr_start = 0xA000 as u16;
    let addr_end = if header_info.ram_size < 0x2000 {
        0xA000 + header_info.ram_size as u16
    } else {
        0xC000 as u16
    };
    let bank_len = (addr_end - addr_start) as usize;
    println!("addr_end = {}", addr_end);
    let mut reply = vec![0; 1];
    for bank in 0..header_info.ram_banks {
        if bank != 0 {
            println!("Switching to ROM bank {:03}", bank);
            switch_bank(&mut port, &header_info.mem_controller, &Memory::Ram, bank)?;
        }

        loop {
            port.write_all(
                cmd_write_raw(addr_start, addr_end).as_slice(),
            )?;
            port.flush()?;

            port.read_exact(&mut reply)?;
            if reply[0] == CmdReply::DMAReady as u8 {
                break;
            }
            thread::sleep(Duration::from_millis(100));
        }
        println!("Writing RAM bank {:03}...", bank);
        port.write_all(
            &sav[bank * 0x2000..bank * 0x2000 + bank_len],
        )?;
        port.flush()?;
    }

    ram_disable(&mut port)?;

    println!("Waiting for confirmation...");
    port.write_all(cmd_ping().as_slice())?;
    port.flush()?;
    port.read_exact(&mut reply)?;
    if reply[0] == CmdReply::Pong as u8 {
        println!("OK!");
        return Ok(());
    } else {
        return Err(Error::new(ErrorKind::Other, "Unexpected reply to ping"));
    }
}

fn write_flash<T: SerialPort>(
    mut port: &mut BufStream<T>,
    //mut file: &File,
    rom: &[u8],
) -> Result<(), io::Error> {

    let header_info = match parse_header(rom) {
        Ok(header_info) => header_info,
        Err(e) => {
            return Err(Error::new(
                ErrorKind::Other,
                format!("Error parsing rom header: {:?}", e),
            ));
        }
    };

    println!("ROM header info:");
    println!();
    print_header(&header_info);
    println!();

    match header_info.mem_controller {
        MemController::None => (),
        MemController::Mbc1 => {
            if header_info.rom_banks > 0x20 {
                return Err(Error::new(
                    ErrorKind::Other,
                    format!(
                        "MBC1 is only MBC5-compatible with max {} banks, but this rom has {} banks",
                        0x1f,
                        header_info.rom_banks
                    ),
                ));
            }
        }
        MemController::Mbc5 => (),
        ref mc => {
            return Err(Error::new(
                ErrorKind::Other,
                format!("{:?} is not MBC5-compatible", mc),
            ));
        }
    }

    erase_flash(&mut port)?;
    println!();

    let mut reply = vec![0; 1];
    for bank in 0..header_info.rom_banks {
        if bank != 0 {
            println!("Switching to ROM bank {:03}", bank);
            switch_bank(&mut port, &MemController::Mbc5, &Memory::Rom, bank)?;
        }
        let addr_start = if bank == 0 { 0x0000 } else { 0x4000 };

        loop {
            port.write_all(
                cmd_write_flash(addr_start, addr_start + 0x4000)
                    .as_slice(),
            )?;
            port.flush()?;

            port.read_exact(&mut reply)?;
            if reply[0] == CmdReply::DMAReady as u8 {
                break;
            }
            thread::sleep(Duration::from_millis(100));
        }
        println!("Writing ROM bank {:03}...", bank);
        port.write_all(&rom[bank * 0x4000..bank * 0x4000 + 0x4000])?;
        port.flush()?;
    }

    println!("Waiting for confirmation...");
    port.write_all(cmd_ping().as_slice())?;
    port.flush()?;
    port.read_exact(&mut reply)?;
    if reply[0] == CmdReply::Pong as u8 {
        println!("OK!");
        return Ok(());
    } else {
        return Err(Error::new(ErrorKind::Other, "Unexpected reply to ping"));
    }
}

fn test3<T: SerialPort>(mut port: &mut BufStream<T>) -> Result<(), io::Error> {
    let mut reply = vec![0; 1];
    println!("Awaiting confirmation...");
    port.write_all(
        vec![0x06, 0x00, 0x00, 0x00, 0x00].as_slice(),
    )?;
    port.flush()?;
    port.read_exact(&mut reply)?;
    if reply[0] == CmdReply::Pong as u8 {
        println!("OK!");
        return Ok(());
    } else {
        return Err(Error::new(ErrorKind::Other, "Unexpected reply to ping"));
    }
}
