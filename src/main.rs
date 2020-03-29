#![allow(dead_code)]
#![allow(unused_variables)]

#[macro_use]
extern crate urpc;

#[macro_use]
extern crate enum_primitive;
extern crate bufstream;
extern crate clap;
extern crate log;
extern crate num;
extern crate pbr;
extern crate serde;
extern crate serial;
extern crate time;
extern crate zip;

extern crate gb_rw_host;

use std::io::{self, ErrorKind};
// use std::io::{BufReader, BufWriter};
use std::cmp;
use std::ffi::OsStr;
use std::fs;
use std::path::Path;
use std::process::{self, Command};
use std::thread;
use std::time::{Duration, Instant};

use urpc::{
    client::{self, RpcClientIOError},
    consts, OptBufNo, OptBufYes,
};

use bufstream::BufStream;
use clap::{App, Arg, ArgMatches, SubCommand};
use log::{error, info, warn};
use num::iter::range_step;
use pbr::{ProgressBar, Units};
use serial::prelude::*;
use std::io::prelude::*;
use zip::{read::ZipArchive, result::ZipError};

use gb_rw_host::{header::*, utils::*};

#[derive(Debug)]
pub enum Error {
    Serial(io::Error),
    Zip(ZipError),
    Urpc(client::Error),
    Generic(String),
}

impl From<io::Error> for Error {
    fn from(err: io::Error) -> Self {
        Self::Serial(err)
    }
}

impl From<ZipError> for Error {
    fn from(err: ZipError) -> Self {
        Self::Zip(err)
    }
}

impl From<RpcClientIOError> for Error {
    fn from(err: RpcClientIOError) -> Self {
        match err {
            RpcClientIOError::Io(err) => Self::Serial(err),
            RpcClientIOError::Urpc(err) => Self::Urpc(err),
        }
    }
}

#[derive(Debug, PartialEq)]
enum Board {
    Generic,
    St,
}

#[derive(Debug, PartialEq)]
enum Memory {
    Rom,
    Ram,
}

mod rpc {

    use serde::Serialize;
    #[derive(Debug, Serialize)]
    pub enum ReqMode {
        GB,
        GBA,
    }

    rpc_client_io! {
        Client;
        client_requests;
        (0, ping, Ping([u8; 4], OptBufNo, [u8; 4], OptBufNo)),
        (1, send_bytes, SendBytes((), OptBufYes, (), OptBufNo)),
        (2, add, Add((u8, u8), OptBufNo, u8, OptBufNo)),
        (3, recv_bytes, RecvBytes((), OptBufNo, (), OptBufYes)),
        (4, send_recv, SendRecv((), OptBufYes, (), OptBufYes)),
        (5, gb_read, GBRead((u16, u16), OptBufNo, (), OptBufYes)),
        (6, mode, Mode(ReqMode, OptBufNo, bool, OptBufNo)),
        (7, gb_write_word, GBWriteWord((u16, u8), OptBufNo, (), OptBufNo)),
        (8, gb_write, GBWrite(u16, OptBufYes, (), OptBufNo)),
        (9, gb_flash_erase, GBFlashErase((), OptBufNo, (), OptBufNo)),
        (10, gb_flash_write, GBFlashWrite(u16, OptBufYes, Option<(u16, u8)>, OptBufNo)),
        (11, gb_flash_erase_sector, GBFlashEraseSector(u16, OptBufNo, bool, OptBufNo)),
        (12, gb_flash_info, GBFlashInfo((), OptBufNo, (u8, u8), OptBufNo)),
        (13, gba_read, GBARead((u32, u16), OptBufNo, (), OptBufYes))
    }
}

fn main() {
    if let None = std::env::var_os(env_logger::DEFAULT_FILTER_ENV) {
        std::env::set_var(env_logger::DEFAULT_FILTER_ENV, "info");
    }
    env_logger::Builder::from_default_env()
        .format_timestamp(None)
        .init();
    let arg_file = Arg::with_name("file")
        .help("Set the file to read/write for the cartridge ROM/RAM")
        .short("f")
        .long("file")
        .value_name("FILE")
        .takes_value(true)
        .required(true);
    let mut app = App::new("gb-rw")
        .version("0.1")
        .about("Gameboy cartridge read/writer")
        .author("Dhole")
        .arg(
            Arg::with_name("baud")
                .help("Set the baud rate")
                .short("b")
                .long("baud")
                .value_name("RATE")
                // Available speeds:
                // https://github.com/torvalds/linux/blob/701a9c8092ddf299d7f90ab2d66b19b4526d1186/drivers/tty/tty_baudrate.c#L26
                // .default_value("1152000")
                // .default_value("921600")
                .default_value("1500000")
                // .default_value("1228800")
                // .default_value("3000000")
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
            Arg::with_name("no-reset")
                .help("Don't reset the development board")
                .short("n"),
        )
        .subcommands(vec![
            SubCommand::with_name("gb")
                .about("Gameboy functions")
                .subcommands(vec![
                    SubCommand::with_name("read-rom")
                        .about("read Gameboy ROM")
                        .arg(arg_file.clone()),
                    SubCommand::with_name("read-ram")
                        .about("read Gameboy RAM")
                        .arg(arg_file.clone()),
                    SubCommand::with_name("write-rom")
                        .about("write Gameboy Flash ROM")
                        .arg(
                            Arg::with_name("erase")
                                .help("Flash erase mode: skip, sector, full")
                                .long("erase")
                                .short("e")
                                .value_name("MODE")
                                .default_value("full")
                                .takes_value(true)
                                .required(false)
                                .validator(|mode| match mode.as_str() {
                                    "skip" => Ok(()),
                                    "sector" => Ok(()),
                                    "full" => Ok(()),
                                    mode => Err(format!("Invalid erase mode: {}", mode)),
                                }),
                        )
                        .arg(arg_file.clone()),
                    SubCommand::with_name("write-ram")
                        .about("write Gameboy RAM")
                        .arg(arg_file.clone()),
                    SubCommand::with_name("erase").about("erase Gameboy Flash ROM"),
                    SubCommand::with_name("flash-info").about("Gameboy Flash chip info"),
                    SubCommand::with_name("read")
                        .about("read Gameboy ROM test")
                        .arg(arg_file.clone()),
                    SubCommand::with_name("new_test")
                        .about("test urpc")
                        .arg(arg_file.clone()),
                ]),
            SubCommand::with_name("gba")
                .about("Gameboy Advance functions")
                .subcommands(vec![
                    SubCommand::with_name("read-rom")
                        .about("read Gameboy Advance ROM")
                        .arg(arg_file.clone())
                        .arg(
                            Arg::with_name("size")
                                .help("ROM size in MB")
                                .short("s")
                                .long("size")
                                .value_name("SIZE")
                                .takes_value(true)
                                .required(false)
                                .validator(|size| match size.parse::<u32>() {
                                    Ok(_) => Ok(()),
                                    Err(e) => Err(format!("{}", e)),
                                }),
                        ),
                    SubCommand::with_name("read-test").about("read Gameboy Advance ROM test"),
                    SubCommand::with_name("write-test").about("write Gameboy Advance ROM test"),
                    SubCommand::with_name("write-rom-test")
                        .about("write Gameboy Advance ROM test")
                        .arg(arg_file.clone()),
                ]),
        ]);
    let matches = app.clone().get_matches();
    if matches.subcommand_name() == None {
        app.print_help().unwrap();
        println!();
        return;
    }

    run_subcommand(matches).unwrap_or_else(|e| {
        error!("Error during operation: {:?}", e);
        process::exit(1);
    });
}

struct Device<T: SerialPort> {
    port: BufStream<T>,
    board: Board,
}

impl<T: SerialPort> Device<T> {
    fn new(port: T, board: Board) -> Self {
        Self {
            port: BufStream::new(port),
            board,
        }
    }

    fn port_clear(&mut self) -> Result<(), io::Error> {
        let timeout = self.port.get_ref().timeout();
        self.port
            .get_mut()
            .set_timeout(Duration::from_millis(100))?;
        let mut buf = [0; 16];
        loop {
            match self.port.read(&mut buf) {
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
        self.port.get_mut().set_timeout(timeout)?;
        Ok(())
    }

    fn reset(&mut self) -> Result<(), Error> {
        match self.board {
            Board::Generic => {
                println!("Press the reset button on the board");
            }
            Board::St => {
                info!("Resetting board using st-flash utility...");
                let output = Command::new("st-flash").arg("reset").output()?;
                info!("\n{}", String::from_utf8_lossy(&output.stderr));
                if !output.status.success() {
                    return Err(Error::Generic(format!(
                        "st-flash returned with error code {:?}",
                        output.status.code()
                    )));
                }
            }
        }
        let mut buf = Vec::new();
        loop {
            self.port.read_until(b'\n', &mut buf)?;
            if buf == b"HELLO\n" {
                break;
            }
            buf.clear();
        }
        Ok(())
    }

    fn send(&mut self, buf: &[u8]) -> Result<(), Error> {
        self.port.write_all(buf)?;
        self.port.flush()?;
        Ok(())
    }

    fn recv(&mut self, buf: &mut [u8]) -> Result<(), Error> {
        self.port.read_exact(buf)?;
        self.port.flush()?;
        Ok(())
    }

    fn wait_cmd_ack(&mut self) -> Result<(), Error> {
        let mut ack = [0];
        self.recv(&mut ack[..])?;
        if ack[0] != 0x41 {
            return Err(Error::Generic(format!(
                "Byte recieved isn't ACK but {:02x}",
                ack[0]
            )));
        }
        Ok(())
    }

    fn mode_gba(&mut self) -> Result<(), Error> {
        self.send(&cmd_mode_gba())?;
        self.wait_cmd_ack()
    }
}

impl<T: SerialPort> io::Read for Device<T> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        self.port.read(buf)
    }
}

impl<T: SerialPort> io::Write for Device<T> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.port.write(buf)
    }
    fn flush(&mut self) -> io::Result<()> {
        self.port.flush()
    }
}

struct Gba<S: io::Read + io::Write> {
    rpc_cli: rpc::Client<S>,
}

impl<S: io::Read + io::Write> Gba<S> {
    fn new(mut rpc_client: rpc::Client<S>) -> Result<Self, Error> {
        rpc_client.mode(rpc::ReqMode::GBA)?;
        info!("GBA Mode Set");
        Ok(Self {
            rpc_cli: rpc_client,
        })
    }

    fn guess_rom_size(&mut self) -> Result<u32, Error> {
        let size = 0x08;
        let empty = [0x00, 0x20, 0x00, 0x20, 0x00, 0x20, 0x00, 0x20];
        for size_mb in &[32, 16, 8, 4] {
            // self.rpc_cli.gba_read((0x0, 8))?;
            // for size_mb in &[16, 8, 4, 8, 16, 32] {
            // for size_mb in &[4u32, 8, 16, 32u32, 16, 8, 4] {
            for end_mb in &[*size_mb, *size_mb - *size_mb / 4] {
                // if *size_mb == 32 && *end_mb == *size_mb {
                //     continue;
                // }
                let addr = end_mb * 1024 * 1024 - size;
                let (_, buf) = self.rpc_cli.gba_read((addr, size as u16))?;

                // print_hex(&buf, addr);
                if buf != empty {
                    return Ok(*size_mb);
                }
            }
        }
        Err(Error::Generic(format!("Unable to guess gba rom size")))
    }

    fn read(&mut self, mut file: &fs::File, size: Option<u32>) -> Result<(), Error> {
        // let (_, buf) = self.rpc_cli.gba_read((0x00_00_00_00, 0x4000))?;
        // print_hex(&buf[..0x100], 0);

        let size_mb = match size {
            None => {
                let s = self.guess_rom_size()?;
                info!("Guessed ROM size: {}MB ({}MBits)", s, s * 8);
                s
            }
            Some(s) => s,
        };

        let size = 0x4000 as u16;
        let n: u32 = size_mb * 1024 * 1024;

        let start = Instant::now();
        let mut pb = new_progress_bar(n as u64);
        for addr in (0x00_00_00_00..n).step_by(size as usize) {
            let (_, buf) = self.rpc_cli.gba_read((addr, size))?;
            file.write_all(&buf)?;
            pb.add(size as u64);
        }
        progress_bar_finish(&mut pb, "Reading ROM", n as usize, start.elapsed());
        Ok(())
    }

    // fn read(&mut self, addr: u32, buf: &mut [u8]) -> Result<(), Error> {
    //     self.dev
    //         .send(&cmd_gba_read(addr, addr + buf.len() as u32))?;
    //     self.dev.recv(&mut buf[..])?;
    //     self.dev.wait_cmd_ack()
    // }

    // fn read_word(&mut self, addr: u32) -> Result<u16, Error> {
    //     let mut buf = [0; 2];
    //     self.read(addr, &mut buf[..])?;
    //     Ok(u16::from_le_bytes(buf))
    // }

    // fn write_word(&mut self, addr: u32, data: u16) -> Result<(), Error> {
    //     self.dev.send(&cmd_gba_write(addr, data))?;
    //     self.dev.wait_cmd_ack()
    // }

    // fn flash_unlock_sector(&mut self, flash_type: FlashType, sector: u32) -> Result<(), Error> {
    //     match flash_type {
    //         FlashType::F3 => {
    //             self.write_word(sector, 0xff)?;
    //             self.write_word(sector, 0x60)?;
    //             self.write_word(sector, 0xd0)?;
    //             self.write_word(sector, 0x90)?;

    //             while self.read_word(sector + 2)? & 0x03 != 0x00 {}
    //             Ok(())
    //         }
    //     }
    // }

    // fn flash_erase_sector(&mut self, flash_type: FlashType, sector: u32) -> Result<(), Error> {
    //     match flash_type {
    //         FlashType::F3 => {
    //             self.write_word(sector, 0xff)?;
    //             self.write_word(sector, 0x20)?;
    //             self.write_word(sector, 0xd0)?;
    //             while self.read_word(sector)? != 0x80 {
    //                 thread::sleep(Duration::from_millis(100));
    //             }
    //             self.write_word(sector, 0xff)?;
    //             Ok(())
    //         }
    //     }
    // }

    // fn flash_write(&mut self, flash_type: FlashType, addr: u32, data: &[u8]) -> Result<(), Error> {
    //     self.dev.send(&cmd_gba_flash_write(
    //         addr,
    //         addr + data.len() as u32,
    //         flash_type,
    //     ))?;
    //     self.dev.send(data)?;
    //     self.dev.wait_cmd_ack()
    // }
}

struct Gb<S: io::Read + io::Write, I> {
    rpc_cli: rpc::Client<S>,
    info: I,
    bank0: Vec<u8>,
}

fn new_progress_bar(n: u64) -> ProgressBar<io::Stdout> {
    let mut pb = ProgressBar::new(n);
    pb.set_units(Units::Bytes);
    pb.format("[=> ]");
    pb
}

fn progress_bar_finish<T: io::Write>(
    pb: &mut ProgressBar<T>,
    action: &str,
    n: usize,
    dur: Duration,
) {
    let dur_millis = cmp::max(dur.as_secs() * 1000 + dur.subsec_millis() as u64, 1);
    pb.finish();
    info!(
        "{} finished in {}m {}s at {:.02} KB/s",
        action,
        dur_millis / 1000 / 60,
        (dur_millis / 1000) % 60,
        n as f64 / 1024.0 / (dur_millis as f64 / 1000.0)
    );
}

enum EraseMode {
    Skip,
    Sector,
    Full,
}

impl<S: io::Read + io::Write> Gb<S, ()> {
    fn new(mut rpc_client: rpc::Client<S>) -> Result<Self, Error> {
        rpc_client.mode(rpc::ReqMode::GB)?;
        info!("GB Mode Set");

        Ok(Self {
            rpc_cli: rpc_client,
            info: (),
            bank0: Vec::new(),
        })
    }
}

impl<S: io::Read + io::Write, I> Gb<S, I> {
    fn with_info_cart(mut self) -> Result<Gb<S, HeaderInfo>, Error> {
        info!("Reading ROM bank 000\n");
        let (_, bank0) = self.rpc_cli.gb_read((0x0000u16, ROM_BANK_SIZE))?;
        let info = self.load_info(&bank0)?;
        Ok(Gb {
            rpc_cli: self.rpc_cli,
            info,
            bank0,
        })
    }

    fn with_info_rom(mut self, rom: &[u8]) -> Result<Gb<S, HeaderInfo>, Error> {
        let mut bank0 = Vec::new();
        let n = cmp::min(rom.len(), ROM_BANK_SIZE as usize);
        bank0.extend_from_slice(&rom[..n]);
        let info = self.load_info(&bank0)?;
        Ok(Gb {
            rpc_cli: self.rpc_cli,
            info,
            bank0,
        })
    }

    fn load_info(&mut self, bank0: &[u8]) -> Result<HeaderInfo, Error> {
        let info = match parse_header(&bank0) {
            Ok(header_info) => header_info,
            Err(e) => {
                print_hex(&bank0[..0x200], 0x0000);
                return Err(Error::Generic(format!(
                    "Error parsing cartridge header: {:?}",
                    e
                )));
            }
        };
        info!("\n{}", info);
        let checksum = header_checksum(&bank0);
        if info.checksum != checksum {
            return Err(Error::Generic(format!(
                "Header checksum mismatch: {:02x} != {:02x}",
                info.checksum, checksum
            )));
        }
        Ok(info)
    }

    fn switch_bank_mc(&mut self, mc: &MemController, mem: &Memory, bank: u16) -> Result<(), Error> {
        let mut writes: Vec<(u16, u8)> = Vec::new();
        match mc {
            MemController::None => {
                if bank != 1 {
                    return Err(Error::Generic(format!(
                        "ROM Only cartridges can't select bank"
                    )));
                }
            }
            MemController::Mbc1 => {
                match mem {
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
                match mem {
                    Memory::Rom => {
                        writes.push((0x2000, ((bank) & 0x00ff) as u8)); // Set bank bits 0..7
                        writes.push((0x3000, (((bank) & 0x0100) >> 8) as u8));
                        // Set bank bit 8
                    }
                    Memory::Ram => {
                        writes.push((0x4000, (bank as u8) & 0x0f)); // Set bank bits 0..4
                    }
                }
            }
            mc => {
                return Err(Error::Generic(format!(
                    "Error: Memory controller {:?} not implemented yet",
                    mc
                )));
            }
        }
        for (addr, data) in writes {
            self.rpc_cli.gb_write_word((addr, data))?;
        }
        Ok(())
    }

    fn ram_enable(&mut self) -> Result<(), Error> {
        Ok(self.rpc_cli.gb_write_word((0x0000, 0x0A))?)
    }

    fn ram_disable(&mut self) -> Result<(), Error> {
        Ok(self.rpc_cli.gb_write_word((0x0000, 0x00))?)
    }

    // Returns (Manufacturer ID, Device ID)
    fn flash_info(&mut self) -> Result<(u8, u8), Error> {
        // for (addr, data) in [(0x0AAA, 0xA9), (0x0555, 0x56), (0x0AAA, 0x90)].iter() {
        //     self.rpc_cli.gb_write_word((*addr, *data))?;
        // }
        // let (_, buf) = self.rpc_cli.gb_read((0x0000, 1))?;
        // let manufacturer_id = buf[0];
        // let (_, buf) = self.rpc_cli.gb_read((0x0002, 1))?;
        // let device_id = buf[0];
        // // self.rpc_cli.gb_write_word((0x0000, 0xFF))?; // Reset
        // Ok((manufacturer_id, device_id))
        Ok(self.rpc_cli.gb_flash_info(())?)
    }

    fn flash_erase(&mut self) -> Result<(), Error> {
        let start = Instant::now();
        self.switch_bank_mc(&MemController::Mbc5, &Memory::Rom, 1)?;

        let writes = vec![
            (0x0AAA, 0xA9),
            (0x0555, 0x56),
            (0x0AAA, 0x80),
            (0x0AAA, 0xA9),
            (0x0555, 0x56),
            (0x0AAA, 0x10), // All
                            //(0x0000, 0x30), // First segment
        ];

        info!("Erasing flash, wait...");
        for (addr, data) in writes {
            self.rpc_cli.gb_write_word((addr, data))?;
        }

        loop {
            thread::sleep(Duration::from_millis(500));
            let (_, buf) = self.rpc_cli.gb_read((0x0000, 1))?;
            if buf[0] == 0xFF {
                break;
            }
            if buf[0] != 0x4c && buf[0] != 0x08 {
                return Err(Error::Generic(format!(
                    "Received incorrect erasing status 0x{:02x}, check the cartridge connection",
                    buf[0]
                )));
            }
        }
        self.rpc_cli.gb_write_word((0x0000, 0xFF))?; // Reset

        let dur = start.elapsed();
        let dur_millis = dur.as_secs() * 1000 + dur.subsec_millis() as u64;
        info!(
            "Flash erased in {}m {}s",
            dur_millis / 1000 / 60,
            (dur_millis / 1000) % 60,
        );
        Ok(())
    }

    fn flash_erase_sector(&mut self, addr: u16) -> Result<(), Error> {
        let writes = vec![
            (0x0AAA, 0xA9),
            (0x0555, 0x56),
            (0x0AAA, 0x80),
            (0x0AAA, 0xA9),
            (0x0555, 0x56),
            (addr, 0x30),
        ];

        for (addr, data) in writes {
            self.rpc_cli.gb_write_word((addr, data))?;
        }

        loop {
            thread::sleep(Duration::from_millis(100));
            let (_, buf) = self.rpc_cli.gb_read((addr, 1))?;
            if buf[0] == 0xFF {
                break;
            }
            if buf[0] != 0x4c && buf[0] != 0x08 {
                return Err(Error::Generic(format!(
                    "Received incorrect erasing status 0x{:02x}, check the cartridge connection",
                    buf[0]
                )));
            }
        }
        // self.rpc_cli.gb_write_word((0x0000, 0xFF))?; // Reset

        Ok(())
    }

    fn flash_write(&mut self, rom: &[u8], erase: EraseMode) -> Result<(), Error> {
        let info = self.load_info(rom)?;
        match info.mem_controller {
            MemController::None => (),
            MemController::Mbc1 => {
                if info.rom_banks > 0x20 {
                    return Err(Error::Generic(format!(
                        "MBC1 is only MBC5-compatible with max {} banks, but this rom has {} banks",
                        0x1f, info.rom_banks
                    )));
                }
            }
            MemController::Mbc5 => (),
            ref mc => {
                return Err(Error::Generic(format!("{:?} is not MBC5-compatible", mc)));
            }
        }

        if let EraseMode::Full = erase {
            self.flash_erase()?;
        }
        const SECTOR_SIZE: u16 = 0x2000;

        let n = info.rom_banks as usize * ROM_BANK_SIZE as usize;
        let mut pb = new_progress_bar(n as u64);
        let start = Instant::now();
        for bank in 0..info.rom_banks {
            if bank != 0 {
                self.switch_bank_mc(&MemController::Mbc5, &Memory::Rom, bank)?;
            }
            let addr = if bank == 0 { 0x0000 } else { ROM_BANK_SIZE };
            if let EraseMode::Sector = erase {
                for i in 0..ROM_BANK_SIZE / SECTOR_SIZE {
                    self.flash_erase_sector(addr + i * SECTOR_SIZE)?;
                }
            }
            let rom_pos = (bank as usize * ROM_BANK_SIZE as usize);
            let mut i = 0;
            loop {
                let fail = self.rpc_cli.gb_flash_write(
                    addr + i,
                    &rom[rom_pos + i as usize..rom_pos + ROM_BANK_SIZE as usize],
                )?;
                if let Some((fail_addr, fail_byte)) = fail {
                    warn!(
                        "gb_flash_write write error at address 0x{:04x}.  Expected 0x{:02x}, got 0x{:02x}",
                        fail_addr,
                        rom[rom_pos + fail_addr as usize],
                        fail_byte,
                    );
                    i = (fail_addr - addr) + 1;
                    if i == ROM_BANK_SIZE {
                        break;
                    }
                } else {
                    break;
                }
            }
            // if !ok {
            //     return Err(Error::Generic(format!(
            //         "gb_flash_write write byte polling timeout"
            //     )));
            // }
            pb.add(ROM_BANK_SIZE as u64);
        }
        let n = info.rom_banks as usize * ROM_BANK_SIZE as usize;
        progress_bar_finish(&mut pb, "Flashing ROM", n, start.elapsed());

        Ok(())
    }
}

impl<S: io::Read + io::Write> Gb<S, HeaderInfo> {
    fn read(&mut self, mut file: &fs::File, memory: Memory) -> Result<(), Error> {
        let start = Instant::now();

        let mut mem = Vec::new();
        let (mut pb, n, action) = match memory {
            Memory::Rom => {
                let n = ROM_BANK_SIZE as usize * self.info.rom_banks as usize;
                let mut pb = new_progress_bar(n as u64);
                mem.extend_from_slice(&self.bank0);
                let addr = 0x4000u16;
                for bank in 1..self.info.rom_banks {
                    self.switch_bank(&memory, bank)?;
                    let (_, buf) = self.rpc_cli.gb_read((addr, ROM_BANK_SIZE))?;
                    mem.extend_from_slice(&buf);
                    pb.add(ROM_BANK_SIZE as u64);
                }
                (pb, n, "Reading ROM")
            }
            Memory::Ram => {
                let n = self.info.ram_size;
                let mut pb = new_progress_bar(n as u64);
                self.ram_enable()?;
                let addr = 0xA000u16;
                let size = cmp::min(self.info.ram_size as u16, RAM_BANK_SIZE);
                for bank in 0..self.info.ram_banks {
                    self.switch_bank(&memory, bank)?;
                    let (_, buf) = self.rpc_cli.gb_read((addr, size))?;
                    mem.extend_from_slice(&buf);
                    pb.add(size as u64);
                }
                self.ram_disable()?;
                (pb, n as usize, "Reading RAM")
            }
        };

        progress_bar_finish(&mut pb, action, n, start.elapsed());
        if let Memory::Rom = memory {
            let global_checksum = global_checksum(&mem);
            if self.info.global_checksum != global_checksum {
                warn!(
                    "Global checksum mismatch: {:04x} != {:04x}",
                    self.info.global_checksum, global_checksum
                );
            } else {
                info!("Global checksum verification successfull!");
            }
        }
        info!("Writing file...");
        file.write_all(&mem)?;
        Ok(())
    }

    fn switch_bank(&mut self, mem: &Memory, bank: u16) -> Result<(), Error> {
        let mc = self.info.mem_controller;
        self.switch_bank_mc(&mc, &mem, bank)
    }

    fn write_ram(&mut self, sav: &[u8]) -> Result<(), Error> {
        let start = Instant::now();

        if sav.len() != self.info.ram_size {
            return Err(Error::Generic(format!(
                "RAM size mismatch between header and file: {:02x} != {:02x}",
                self.info.ram_size,
                sav.len()
            )));
        }

        self.ram_enable()?;
        let addr = 0xA000u16;
        let size = cmp::min(self.info.ram_size as u16, RAM_BANK_SIZE);
        let n = RAM_BANK_SIZE as usize * self.info.ram_banks as usize;
        let mut pb = new_progress_bar(n as u64);
        for bank in 0..self.info.ram_banks {
            self.switch_bank(&Memory::Ram, bank)?;
            let sav_pos = bank as usize * RAM_BANK_SIZE as usize;
            self.rpc_cli
                .gb_write(addr, &sav[sav_pos..sav_pos + size as usize])?;
            pb.add(RAM_BANK_SIZE as u64);
        }
        progress_bar_finish(&mut pb, "Writtng RAM", n, start.elapsed());

        Ok(self.ram_disable()?)
    }
}

enum_from_primitive! {
#[derive(Debug, PartialEq)]
enum FlashType {
    F3 = 3,
}
}

enum_from_primitive! {
#[derive(Debug, PartialEq)]
enum Cmd {
    Read = 0,
    Write = 1,
    WriteRaw = 2,
    WriteFlash = 3,
    Erase = 4,
    Reset = 5,
    Ping = 6,
    ModeGBA = 7,
    ModeGB = 8,
    ReadGBAROM = 9,
    WriteGBAROM = 10,
    WriteGBAFlash = 11,
    ReadGBAWord = 12,
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

fn cmd_mode_gba() -> [u8; 1] {
    [Cmd::ModeGBA as u8]
}

fn cmd_gba_read(addr_start: u32, addr_end: u32) -> [u8; 7] {
    let start = (addr_start >> 1).to_le_bytes();
    // We send the last addr_end we ask for because otherwise for roms of 32MB we overflow the 24
    // bits
    let end = ((addr_end >> 1) - 1).to_le_bytes();
    [
        Cmd::ReadGBAROM as u8,
        start[0],
        start[1],
        start[2],
        end[0],
        end[1],
        end[2],
    ]
}

fn cmd_gba_read_word(addr: u32) -> [u8; 4] {
    let addr = (addr >> 1).to_le_bytes();
    [Cmd::ReadGBAWord as u8, addr[0], addr[1], addr[2]]
}

fn cmd_gba_write(addr: u32, data: u16) -> [u8; 6] {
    let addr = (addr >> 1).to_le_bytes();
    let data = data.to_le_bytes();
    [
        Cmd::WriteGBAROM as u8,
        addr[0],
        addr[1],
        addr[2],
        data[0],
        data[1],
    ]
}

fn cmd_gba_flash_write(addr_start: u32, addr_end: u32, flash_type: FlashType) -> [u8; 8] {
    let start = (addr_start >> 1).to_le_bytes();
    let end = ((addr_end >> 1) - 1).to_le_bytes();
    [
        Cmd::WriteGBAFlash as u8,
        start[0],
        start[1],
        start[2],
        end[0],
        end[1],
        end[2],
        flash_type as u8,
    ]
}

enum FilePath<'a> {
    Read(&'a str),
    Write(&'a str),
    None,
}

enum File<'a> {
    Read(&'a Path, Vec<u8>),
    Write(&'a Path, fs::File),
    None,
}

fn run_subcommand(matches: ArgMatches) -> Result<(), Error> {
    let serial = matches.value_of("serial").unwrap();
    let baud = matches.value_of("baud").unwrap().parse::<usize>().unwrap();
    let board = match matches.value_of("board").unwrap() {
        "generic" => Board::Generic,
        "st" => Board::St,
        board => return Err(Error::Generic(format!("Invalid board: {}", board))),
    };
    let reset = !matches.is_present("no-reset");

    info!("Development board is: {:?}", board);
    info!("Using serial device: {} at baud rate: {}", serial, baud);

    let mut port_raw = match serial::open(serial) {
        Ok(port) => port,
        Err(e) => {
            error!("Error opening {}: {}", serial, e);
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
            error!("Error configuring {}: {}", serial, e);
            process::exit(1);
        });
    port_raw
        .set_timeout(Duration::from_secs(16))
        .unwrap_or_else(|e| {
            error!("Error setting timeout for {}: {}", serial, e);
            process::exit(1);
        });
    let mut device = Device::new(port_raw, board);
    device.port_clear()?;

    if reset {
        device.reset().unwrap_or_else(|e| {
            error!("Error resetting development board: {:?}", e);
            process::exit(1);
        });
        info!("Connected!");
    }

    let (mode, cmd, path) = match matches.subcommand() {
        (mode, Some(app)) => match app.subcommand() {
            (cmd, Some(sub)) => (mode, cmd, sub.value_of("file")),
            _ => unreachable!(),
        },
        _ => unreachable!(),
    };
    let file_path = match (mode, cmd, path) {
        ("gb", "read-rom", Some(path)) => FilePath::Write(path),
        ("gb", "read-ram", Some(path)) => FilePath::Write(path),
        ("gba", "read-rom", Some(path)) => FilePath::Write(path),
        ("gba", "read-ram", Some(path)) => FilePath::Write(path),
        ("gb", "write-rom", Some(path)) => FilePath::Read(path),
        ("gb", "write-ram", Some(path)) => FilePath::Read(path),
        ("gba", "write-rom", Some(path)) => FilePath::Read(path),
        ("gba", "write-ram", Some(path)) => FilePath::Read(path),
        _ => FilePath::None,
    };
    let file = match file_path {
        FilePath::Read(path) => {
            let mut content = Vec::new();
            let path = Path::new(path);
            if path.extension() == Some(OsStr::new("zip")) {
                let mut zip = ZipArchive::new(fs::File::open(path)?)?;
                for i in 0..zip.len() {
                    let mut zip_file = zip.by_index(i).unwrap();
                    let filename = zip_file.name().to_string();
                    let extension = Path::new(&filename).extension();
                    if extension == Some(OsStr::new("gb")) || extension == Some(OsStr::new("gbc")) {
                        zip_file.read_to_end(&mut content)?;
                        break;
                    }
                }
                if content.len() == 0 {
                    return Err(Error::Generic(format!(
                        "File {} doesn't contain any ROM",
                        path.display()
                    )));
                }
            } else {
                fs::OpenOptions::new()
                    .read(true)
                    .open(path)?
                    .read_to_end(&mut content)?;
            }
            File::Read(path, content)
        }
        FilePath::Write(path) => {
            let path = Path::new(path);
            File::Write(
                path,
                fs::OpenOptions::new()
                    .write(true)
                    .create_new(true)
                    .open(path)?,
            )
        }
        FilePath::None => File::None,
    };

    // let mut rpc_client = client::RpcClient::new();
    let mut rpc_client = rpc::Client::new(device, 0x4100);

    let result = match matches.subcommand() {
        ("gb", Some(app)) => {
            match (app.subcommand(), &file) {
                (("read-rom", _), File::Write(path, file)) => {
                    let mut gb = Gb::new(rpc_client)?.with_info_cart()?;
                    info!("Reading cartridge ROM into {}", path.display());
                    gb.read(file, Memory::Rom)
                }
                (("read-ram", _), File::Write(path, file)) => {
                    let mut gb = Gb::new(rpc_client)?.with_info_cart()?;
                    info!("Reading cartridge ROM into {}", path.display());
                    gb.read(file, Memory::Ram)
                }
                (("write-rom", Some(app)), File::Read(path, content)) => {
                    let mut gb = Gb::new(rpc_client)?;
                    info!("Writing {} into cartridge ROM", path.display());
                    let erase_mode = match app.value_of("erase").unwrap() {
                        "skip" => EraseMode::Skip,
                        "sector" => EraseMode::Sector,
                        "full" => EraseMode::Full,
                        _ => unreachable!(),
                    };
                    gb.flash_write(&content, erase_mode)
                }
                (("write-ram", _), File::Read(path, content)) => {
                    let mut gb = Gb::new(rpc_client)?.with_info_cart()?;
                    info!("Writing {} into cartridge RAM", path.display());
                    gb.write_ram(&content)
                }
                (("erase", _), _) => {
                    let mut gb = Gb::new(rpc_client)?;
                    info!("Erasing flash");
                    gb.flash_erase()
                }
                (("flash-info", _), _) => {
                    let mut gb = Gb::new(rpc_client)?;
                    let (manufacturer_id, device_id) = gb.flash_info()?;
                    info!(
                        "Manufacturer ID: 0x{:02x}, Device ID: 0x{:02x}",
                        manufacturer_id, device_id
                    );
                    Ok(())
                }
                ((cmd, _), _) => Err(Error::Generic(format!("Unexpected subcommand: {}", cmd))),
                // UPDATE
                // ("read", Some(_)) => read_test(&mut port),
            }
        }
        ("gba", Some(app)) => {
            match (app.subcommand(), &file) {
                (("read-rom", Some(app)), File::Write(path, file)) => {
                    let mut gba = Gba::new(rpc_client)?;
                    info!("Reading cartridge ROM into {}", path.display());
                    let size = app.value_of("size").map(|s| s.parse::<u32>().unwrap());
                    // gba.read(file, size, Memory::Rom)
                    gba.read(file, size)
                    // gba_read_rom(&mut Gba::new(device)?, &file, size)
                }
                // ("read-test", Some(_)) => {
                //     unimplemented!()
                //     // println!("Reading GBA cartridge ROM into stdout");
                //     // gba_read_test(&mut Gba::new(device)?)
                // }
                //("write-test", Some(_)) => {
                //    unimplemented!()
                //    // println!("Writing and reading GBA cartridge data, showing result in stdout");
                //    // gba_write_test(&mut Gba::new(device)?)
                //}
                //("write-ROM-test", Some(_)) => {
                //    unimplemented!()
                //    // println!("Writing {} into cartridge GBA ROM", path.display());
                //    // let mut rom = Vec::new();
                //    // OpenOptions::new()
                //    //     .read(true)
                //    //     .open(path)?
                //    //     .read_to_end(&mut rom)?;
                //    // write_gba_rom_test(&mut Gba::new(device)?, &rom)
                //}
                ((cmd, _), _) => Err(Error::Generic(format!("Unexpected subcommand: {}", cmd))),
            }
        }
        (cmd, _) => Err(Error::Generic(format!("Unexpected subcommand: {}", cmd))),
    };

    // Error cleanup
    if result.is_err() {
        if let File::Write(path, _) = file {
            fs::remove_file(path).unwrap_or(())
        }
    }
    result
}

// fn gba_read_rom<T: SerialPort>(
//     gba: &mut Gba<T>,
//     mut file: &fs::File,
//     size: Option<u32>,
// ) -> Result<(), Error> {
//     let size = match size {
//         None => {
//             let s = guess_gba_rom_size(gba)?;
//             info!("Guessed ROM size: {}MB ({}MBits)", s, s * 8);
//             s
//         }
//         Some(s) => s,
//     };
//
//     let buf_len = 0x8000 as u32;
//     let mut buf = vec![0; buf_len as usize];
//     let end: u32 = size * 1024 * 1024;
//     let mut pb = ProgressBar::new(end as u64);
//     pb.set_units(Units::Bytes);
//     pb.format("[=> ]");
//     for addr in range_step(0x00_00_00_00, end, buf_len) {
//         gba.read(addr, &mut buf)?;
//         file.write_all(&buf)?;
//         pb.add(buf_len as u64);
//     }
//     pb.finish_print("Reading ROM finished");
//     Ok(())
// }
//
// fn gba_read_test<T: SerialPort>(gba: &mut Gba<T>) -> Result<(), Error> {
//     let buf_len = 0x100;
//     let mut buf = vec![0; 0x800000];
//     //for addr_start in range_step(0x1_00_00_00, 0x1_01_00_00, buf_len) {
//     //for addr_start in range_step(0x0, 0x4000 * 1, buf_len) {
//     let start = 0x0000;
//     for addr_start in range_step(start, start + buf_len * 1, buf_len) {
//         let addr_end = addr_start + buf_len;
//         //// READ_GBA_ROM
//         gba.read(addr_start, &mut buf[..(addr_end - addr_start) as usize])?;
//         //// READ_GBA_WORD
//         // for addr in (addr_start..addr_end).step_by(2) {
//         //     run_cmd(port, cmd_gba_read_word(addr).as_slice())?;
//         //     let mut word_le: [u8; 2] = [0, 0];
//         //     port.read_exact(&mut word_le[..])?;
//         //     wait_cmd_ack(port)?;
//         //     buf[addr as usize] = word_le[0];
//         //     buf[addr as usize + 1] = word_le[1];
//         // }
//
//         println!("=== {:08x} - {:08x} ===", addr_start, addr_end);
//         // print_hex(&buf[addr_start as usize..addr_end as usize], addr_start);
//         print_hex(&buf[..(addr_end - addr_start) as usize], addr_start);
//         println!();
//     }
//     Ok(())
// }

fn gba_write_test<T: SerialPort>(gba: &mut Gba<T>) -> Result<(), Error> {
    // port.write_all(cmd_gba_write(0x000000, 0xf0f0).as_slice())?;
    // port.flush()?;

    // port.write_all(cmd_gba_write(0x000154, 0x9898).as_slice())?;
    // port.flush()?;

    // port.write_all(cmd_gba_read(0x000040, 0x000042).as_slice())?;
    // port.flush()?;

    // port.read_exact(&mut buf)?;
    // println!("* read(0x000040) = {:02x}{:02x}", buf[0], buf[1]);

    // port.write_all(cmd_gba_write(0x000000, 0x00ff).as_slice())?;
    // port.flush()?;

    // port.write_all(cmd_gba_write(0x0000aa, 0x0098).as_slice())?;
    // port.flush()?;

    // port.write_all(cmd_gba_write(0x000000, 0xf0f0).as_slice())?;
    // port.flush()?;

    // port.write_all(cmd_gba_write(0x0000aa, 0x9898).as_slice())?;
    // port.flush()?;

    // port.write_all(cmd_gba_read(0x000020, 0x000022).as_slice())?;
    // port.flush()?;

    // port.read_exact(&mut buf)?;
    // println!("* read(0x000020) = {:02x}{:02x}", buf[0], buf[1]);

    // port.write_all(cmd_gba_write(0x000000, 0xf0f0).as_slice())?;
    // port.flush()?;
    // port.write_all(cmd_gba_write(0x000154, 0x9898).as_slice())?;
    // port.flush()?;

    // // *0x8000040 != 0x5152
    // // *0x8000020 != 0x5152

    // port.write_all(cmd_gba_write(0x000000, 0x00f0).as_slice())?;
    // port.flush()?;

    // port.write_all(cmd_gba_write(0x000000, 0x00ff).as_slice())?;
    // port.flush()?;
    // port.write_all(cmd_gba_write(0x000000, 0x0050).as_slice())?;
    // port.flush()?;
    // port.write_all(cmd_gba_write(0x000000, 0x0090).as_slice())?;
    // port.flush()?;

    // *0x8000000 == 0x8a

    // port.write_all(cmd_gba_write(0x000000, 0x00ff).as_slice())?;
    // port.flush()?;
    // port.write_all(cmd_gba_write(0x000000, 0x0090).as_slice())?;
    // port.flush()?;

    // *0x8000002 == 0x8810 -> 3

    // Disable ID mode
    // port.write_all(cmd_gba_write(0x000002, 0x00ff).as_slice())?;
    // port.flush()?;

    let sector = 0x0000;

    //// Erase 0x8000 bytes sector

    // Guess: Unlock sector
    /*
    println!("DBG Unlock sector");
    bus_gba_write(port, sector, 0xff)?;
    bus_gba_write(port, sector, 0x60)?;
    bus_gba_write(port, sector, 0xd0)?;
    bus_gba_write(port, sector, 0x90)?;

    for _ in 0..32 {
        let v = bus_gba_read_word(port, sector + 2)?;
        println!("1 read({:08x}) = {:04x}", sector + 2, v);
        if v & 0x03 == 0 {
            break;
        }
        thread::sleep(Duration::from_millis(50));
    }

    // Erase sector
    println!("DBG Erase sector");
    bus_gba_write(port, sector, 0xff)?;
    bus_gba_write(port, sector, 0x20)?;
    bus_gba_write(port, sector, 0xd0)?;

    loop {
        let v = bus_gba_read_word(port, sector)?;
        println!("2 read({:08x}) = {:04x}", sector, v);
        if v == 0x80 {
            break;
        }
        thread::sleep(Duration::from_millis(50));
    }

    bus_gba_write(port, sector, 0xff)?;
    */

    // gba_flash_a_unlock_sector(port, 0 * 0x8000)?;
    // gba_flash_a_erase_sector(port, 0 * 0x8000)?;
    // for i in 0..5 {
    //     gba_flash_a_unlock_sector(port, i * 0x8000)?;
    //     gba_flash_a_erase_sector(port, i * 0x8000)?;
    //     println!("Erased sector {:08x}", i * 0x8000);
    // }
    // let data = vec![0, 1];
    // for i in 0..4 {
    //     bus_gba_flash_write(port, 0x20000 + i * 0x8000, &data, FlashType::F3)?;
    // }

    // gba.flash_unlock_sector(FlashType::F3, 0xfc0000)?;
    // gba.flash_erase_sector(FlashType::F3, 0xfc0000)?;
    // gba.flash_unlock_sector(FlashType::F3, 0xfc1000)?;
    // gba.flash_erase_sector(FlashType::F3, 0xfc1000)?;

    //// Write data
    // let sector = 0x0000;

    //let data = [0xaa, 0xaa, 0xaa, 0xaa, 0xaa, 0xaa, 0xaa, 0xaa];
    //let data = [0x88, 0x88, 0x88, 0x88, 0x88, 0x88, 0x88, 0x88];
    // let data = [0xcc, 0xcc, 0xcc, 0xcc, 0xcc, 0xcc, 0xcc, 0xcc];
    // let data = [0xee, 0xee, 0xcc, 0xcc, 0xaa, 0xaa, 0x99, 0x99];
    // let data = [0x00, 0x0f, 0xf0, 0xff];
    // let data = [0xff, 0xf0, 0x0f, 0x00];
    // let data = [0xde, 0xad, 0xbe, 0xef];
    // let mut data = vec![0; 2];
    // for i in 0..data.len() {
    //     data[i] = i as u8;
    // }
    // let data = vec![0, 1];

    // let word = bus_gba_read_word(port, sector)?;
    // println!("Before: {:08x} : {:04x}", sector, word);

    // println!("DBG Write flash F3 data");
    // bus_gba_flash_write(port, sector, &data, FlashType::F3)?;

    // let word = bus_gba_read_word(port, sector)?;
    // println!("After: {:08x} : {:04x}", sector, word);
    /*
    for i in 0..data.len() / 2 {
        println!("Write byte");
        let addr = sector + (i * 2) as u32;
        bus_gba_write(port, addr, 0xff)?;
        bus_gba_write(port, addr, 0x70)?;

        loop {
            let v = bus_gba_read_word(port, addr)?;
            println!("3 read({:08x}) = {:04x}", addr, v);
            if v == 0x80 {
                break;
            }
            // thread::sleep(Duration::from_millis(50));
        }

        bus_gba_write(port, addr, 0xff)?;
        bus_gba_write(port, addr, 0x40)?;

        let w = (data[i * 2] as u16) | ((data[i * 2 + 1] as u16) << 8);
        println!("4 write({:08x}) = {:04x}", addr, w);
        bus_gba_write(port, addr, w)?;

        loop {
            let v = bus_gba_read_word(port, addr)?;
            println!("4 read({:08x}) = {:04x}", addr, v);
            if v == 0x80 {
                break;
            }
            // thread::sleep(Duration::from_millis(50));
        }
    }
    bus_gba_write(port, sector, 0xff)?;
    */

    // for (i = 0; i < 0x8000; i++) {
    //   *r2 = 0xff;
    //   *r2 = 0x70;

    //   while (*r2 != 0x80);

    //   *r2 = 0xff;
    //   *r2 = 0x40;

    //   *r2 = (u16) *r5;
    //   while (*r2 != 0x80);

    //   r2 +=2
    //   r5 +=2
    // }

    // *r2 = 0xff;

    Ok(())
}

// fn write_gba_rom_test<T: SerialPort>(gba: &mut Gba<T>, rom: &[u8]) -> Result<(), Error> {
//     let sec_len: usize = 0x8000;
//     if rom.len() % sec_len != 0 {
//         return Err(Error::Generic(format!(
//             "Rom length is not multiple of sector length ({}), it's: {:?}",
//             sec_len,
//             rom.len()
//         )));
//     }
//
//     /*
//     let mut pb = ProgressBar::new(rom.len() as u64);
//     pb.set_units(Units::Bytes);
//     pb.format("[=> ]");
//     for i in 0..rom.len() / sec_len {
//         let sector = (i as u32) * (sec_len as u32);
//         gba_flash_a_unlock_sector(port, sector)?;
//         gba_flash_a_erase_sector(port, sector)?;
//         pb.add(sec_len as u64);
//     }
//     pb.finish_print("Erasing ROM finished");
//     // thread::sleep(Duration::from_millis(1000));
//     */
//
//     // let mut buf = vec![0; sec_len];
//     let mut buf = vec![0; 0x100];
//     // let mut buf2 = vec![0; 0x10];
//     let mut pb = ProgressBar::new(rom.len() as u64);
//     pb.set_units(Units::Bytes);
//     pb.format("[=> ]");
//     for i in 0..rom.len() / sec_len {
//         let sector = (i as u32) * (sec_len as u32);
//         // println!("DBG Unlock");
//         // gba_flash_a_unlock_sector(port, sector)?; // DBG
//         gba.flash_unlock_sector(FlashType::F3, sector)?;
//         for attempt in 0..4 {
//             if i < 4 || i % 4 == 0 {
//                 gba.flash_erase_sector(FlashType::F3, sector)?;
//             }
//             let rom_sector = &rom[sector as usize..sector as usize + sec_len];
//             // if rom_sector.iter().all(|b| *b == 0x00) || rom_sector.iter().all(|b| *b == 0xff) {
//             if rom_sector.iter().all(|b| *b == 0xff) {
//                 break;
//             }
//             // println!("DBG flash write");
//             gba.flash_write(FlashType::F3, sector, rom_sector)?;
//             gba.read(sector, &mut buf)?;
//             if buf[..] == rom[sector as usize..sector as usize + buf.len()] {
//                 break;
//             }
//             println!(
//                 "Sector {:08x} check didn't pass, flashing it again...",
//                 sector
//             );
//             println!("=== Expected ===");
//             print_hex(
//                 &rom[sector as usize..sector as usize + 0x100],
//                 sector as u32,
//             );
//             println!();
//             println!("=== Got ===");
//             print_hex(&buf[..0x100], sector as u32);
//             println!();
//             if attempt == 3 {
//                 return Err(Error::Generic(format!(
//                     "Sector {:08x} check didn't pass after 4 write attempts",
//                     sector
//                 )));
//             }
//         }
//         pb.add(sec_len as u64);
//         // Check that previous sectors were not erased!
//         // for j in 0..i {
//         //     let sector2 = (j as u32) * (sec_len as u32);
//         //     bus_gba_read(port, sector2, sector2 + 0x10 as u32, &mut buf2)?;
//         //     if buf2[..] != rom[sector2 as usize..sector2 as usize + 0x10] {
//         //         println!(
//         //             "While writing sector {:08x}, sector {:08x} was erased!",
//         //             sector, sector2
//         //         );
//         //         println!("=== Expected ===");
//         //         print_hex(
//         //             &rom[sector2 as usize..sector2 as usize + 0x10],
//         //             sector2 as u32,
//         //         );
//         //         println!();
//         //         println!("=== Got ===");
//         //         print_hex(&buf2[..0x10], sector2 as u32);
//         //         println!();
//         //     } else {
//         //         println!("sector {:08x} is good!", sector2);
//         //     }
//         // }
//     }
//     pb.finish_print("Writing ROM finished");
//
//     Ok(())
// }
//
// fn guess_gba_rom_size<T: SerialPort>(gba: &mut Gba<T>) -> Result<u32, Error> {
//     let mut buf = vec![0; 0x08];
//     for size in vec![32, 16, 8, 4] {
//         for end in vec![size, size - size / 4] {
//             let addr_end = end * 1024 * 1024;
//             let addr_start = addr_end - 0x08;
//             gba.read(addr_start, &mut buf)?;
//
//             if buf != vec![0x00, 0x20, 0x00, 0x20, 0x00, 0x20, 0x00, 0x20] {
//                 return Ok(size);
//             }
//         }
//     }
//     Err(Error::Generic(format!("Unable to guess gba rom size")))
// }
