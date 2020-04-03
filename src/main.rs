#![allow(dead_code)]
#![allow(unused_variables)]

mod header;
mod utils;

#[macro_use]
extern crate urpc;

#[macro_use]
extern crate enum_primitive;

use std::cmp;
use std::ffi::OsStr;
use std::fs;
use std::io::{self, ErrorKind};
use std::path::Path;
use std::process::{self, Command};
use std::thread;
use std::time::{Duration, Instant};

use urpc::client::{self, RpcClientIOError};

use bufstream::BufStream;
use clap::{App, Arg, ArgMatches, SubCommand};
use log::{error, info, warn};
use pbr::{ProgressBar, Units};
use serial::prelude::*;
use std::io::prelude::*;
use zip::{read::ZipArchive, result::ZipError};

use crate::{header::*, utils::*};
// use gb_rw_host::{header::*, utils::*};

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

    #[derive(Debug, Clone, Copy, Deserialize)]
    pub struct GBStats {
        flash_write_retries: u64,
        flash_write_errors: u64,
    }

    #[derive(Debug, PartialEq, Serialize)]
    pub enum FlashType {
        F3 = 3,
    }

    rpc_client_io! {
        Client;
        client_requests;
        (0, ping, Ping([u8; 4], OptBufNo, [u8; 4], OptBufNo)),
        (1, mode, Mode(ReqMode, OptBufNo, bool, OptBufNo)),
        (2, gb_read, GBRead((u16, u16), OptBufNo, (), OptBufYes)),
        (3, gb_write_word, GBWriteWord((u16, u8), OptBufNo, (), OptBufNo)),
        (4, gb_write, GBWrite(u16, OptBufYes, (), OptBufNo)),
        // (5, gb_flash_erase, GBFlashErase((), OptBufNo, (), OptBufNo)),
        (6, gb_flash_write, GBFlashWrite(u16, OptBufYes, Option<(u16, u8)>, OptBufNo)),
        (7, gb_flash_erase_sector, GBFlashEraseSector(u16, OptBufNo, bool, OptBufNo)),
        (8, gb_flash_info, GBFlashInfo((), OptBufNo, (u8, u8), OptBufNo)),
        (9, gb_get_stats, GBGetStats((), OptBufNo, Option<GBStats>, OptBufNo)),
        (50, gba_read, GBARead((u32, u16), OptBufNo, (), OptBufYes)),
        (51, gba_write_word, GBAWriteWord((u32, u16), OptBufNo, (), OptBufNo)),
        (52, gba_flash_write, GBAFlashWrite((FlashType, u32), OptBufYes, Option<u32>, OptBufNo)),
        (53, gba_flash_unlock_sector, GBAFlashUnlockSector((FlashType, u32), OptBufNo, bool, OptBufNo)),
        (54, gba_flash_erase_sector, GBAFlashEraseSector((FlashType, u32), OptBufNo, bool, OptBufNo)),
        (255, test1, Test((), OptBufNo, Option<[(u16, u16); 8]>, OptBufNo)) // 1
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
        .version("0.2")
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
                .short("n")
                .long("no-reset"),
        )
        .subcommands(vec![
            SubCommand::with_name("debug")
                .about("Device debug functions")
                .subcommands(vec![SubCommand::with_name("ping").about("Ping device")])
                .subcommands(vec![SubCommand::with_name("gba-test").about("GBA test")]),
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
                    SubCommand::with_name("write-rom")
                        .about("write Gameboy Advance ROM")
                        .arg(arg_file.clone())
                        .arg(
                            Arg::with_name("diff")
                                .help("When flashing, only writte the sectors that differ from this file")
                                .short("d")
                                .long("diff")
                                .value_name("FILE")
                                .takes_value(true)
                        ),
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

    fn read_word(&mut self, addr: u32) -> Result<u16, Error> {
        let (_, buf) = self.rpc_cli.gba_read((addr, 2))?;
        Ok(u16::from_le_bytes([buf[0], buf[1]]))
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

    fn test1(&mut self) -> Result<(), Error> {
        let (_, buf) = self.rpc_cli.gba_read((0, 0x40))?;
        print_hex(&buf, 0);

        let w = self.read_word(0)?;
        println!("0 w: {:04x}", w);
        let w = self.read_word(2)?;
        println!("2 w: {:04x}", w);

        let result = self.rpc_cli.test1(())?.unwrap();
        for (w0, w2) in result.iter() {
            println!("0 w: {:04x} 2 w: {:04x}", w0, w2); // 8815
        }
        let (_, buf) = self.rpc_cli.gba_read((0, 0x40))?;
        print_hex(&buf, 0);

        let w = self.read_word(0)?;
        println!("0 w: {:04x}", w);
        let w = self.read_word(2)?;
        println!("2 w: {:04x}", w);

        self.rpc_cli.gba_write_word((0, 0xff))?; // reset

        let (_, buf) = self.rpc_cli.gba_read((0, 0x40))?;
        print_hex(&buf, 0);

        let w = self.read_word(0)?;
        println!("0 w: {:04x}", w);
        let w = self.read_word(2)?;
        println!("2 w: {:04x}", w);

        self.rpc_cli.gba_write_word((0, 0xff))?; // reset
        Ok(())
    }

    fn test2(&mut self) -> Result<(), Error> {
        // let w = self.read_word(0x8000002)?;
        // println!("1a w: {:04x}", w); // 8815

        // let w = self.read_word(0x8000000)?;
        // println!("0a w: {:02x}", (w & 0x00ff) as u8); // 8a

        // self.rpc_cli.gba_write_word((0x8000000, 0xff))?;
        // self.rpc_cli.gba_write_word((0x8000000, 0x50))?;
        // self.rpc_cli.gba_write_word((0x8000000, 0x90))?;

        // let w = self.read_word(0x8000000)?;
        // println!("0b w: {:02x}", (w & 0x00ff) as u8); // 8a

        // // if (*0x8000000 != 0x8a) {
        // //   *0x8000000 = 0xf0;
        // //   return 2; // r0 = 2; goto 0x08f80380;
        // // }
        // let w = self.read_word(0x8000002)?;
        // println!("1a w: {:04x}", w); // 8815

        // self.rpc_cli.gba_write_word((0x8000000, 0xff))?;
        // self.rpc_cli.gba_write_word((0x8000000, 0x90))?;

        // let w = self.read_word(0x8000002)?;
        // println!("1b w: {:04x}", w); // 8815

        // self.rpc_cli.gba_write_word((0x8000002, 0xff))?;

        // switch (*0x8000002) {
        // case 0x8815:
        // case 0x8810:
        // case 0x880e:
        //   *0x8000002 = 0xff;
        //   return 3 // r0 = 3; goto 0x08f80380;
        // case 0x887d:
        // case 0x88b0:
        //   *0x8000002 = 0xff;
        //   return 1; // r0 = 1; goto 0x08f80380;
        // case 0x227d:
        // default:
        //   *0x8000002 = 0xf0;
        //   return 2; // r0 = 2; goto 0x08f80380;
        //   break;
        // }

        info!("Unlocking sector...");
        let ok = self
            .rpc_cli
            .gba_flash_unlock_sector((rpc::FlashType::F3, 0))?;
        if !ok {
            return Err(Error::Generic(format!("gba_flash_unlock_sector failed")));
        }
        info!("Erasing sector...");
        let ok = self
            .rpc_cli
            .gba_flash_erase_sector((rpc::FlashType::F3, 0))?;
        if !ok {
            return Err(Error::Generic(format!("gba_flash_erase_sector failed")));
        }

        let fail = self.rpc_cli.gba_flash_write(
            (rpc::FlashType::F3, 0),
            &[0xd, 0xe, 0xa, 0xd, 0xb, 0xe, 0xe, 0xf],
        )?;
        info!("Fail: {:?}", fail);

        let (_, buf) = self.rpc_cli.gba_read((0, 0x200))?;
        print_hex(&buf, 0);
        Ok(())
    }

    fn flash_write(&mut self, rom: &[u8], diff: Option<Vec<u8>>) -> Result<(), Error> {
        const SECTOR_SIZE: usize = 0x8000;
        if rom.len() % SECTOR_SIZE != 0 {
            return Err(Error::Generic(format!(
                "Rom length is not multiple of sector size ({}), it's: {:?}",
                SECTOR_SIZE,
                rom.len()
            )));
        }
        if let Some(diff_rom) = &diff {
            if diff_rom.len() != rom.len() {
                return Err(Error::Generic(format!(
                    "Diff rom differs from rom: {} != {}",
                    diff_rom.len(),
                    rom.len()
                )));
            }
        }

        let mut pb = new_progress_bar(rom.len() as u64);
        let start = Instant::now();
        for i in 0..rom.len() / SECTOR_SIZE {
            let sector = (i as u32) * (SECTOR_SIZE as u32);
            let ok = self
                .rpc_cli
                .gba_flash_unlock_sector((rpc::FlashType::F3, sector))?;
            if !ok {
                return Err(Error::Generic(format!(
                    "flash unlock sector failed at sector 0x{:06}",
                    sector
                )));
            }
            if let Some(diff_rom) = &diff {
                let sector = sector / 4 * 4;
                let end = std::cmp::min(sector as usize + SECTOR_SIZE * 4, rom.len());
                let rom_4sectors = &rom[sector as usize..end];
                let diff_4sectors = &diff_rom[sector as usize..end];
                if diff_4sectors == rom_4sectors {
                    pb.add(SECTOR_SIZE as u64);
                    continue;
                }
            }

            let rom_sector = &rom[sector as usize..sector as usize + SECTOR_SIZE];
            for attempt in 0..4 {
                if i < 4 || i % 4 == 0 {
                    let ok = self
                        .rpc_cli
                        .gba_flash_erase_sector((rpc::FlashType::F3, sector))?;
                    if !ok {
                        return Err(Error::Generic(format!(
                            "flash erase sector failed at sector 0x{:06}",
                            sector
                        )));
                    }
                }
                if rom_sector.iter().all(|b| *b == 0xff) {
                    break;
                }
                match self
                    .rpc_cli
                    .gba_flash_write((rpc::FlashType::F3, sector), rom_sector)?
                {
                    None => {}
                    Some(addr) => {
                        return Err(Error::Generic(format!(
                            "flash write failed at address 0x{:06}",
                            addr
                        )));
                    }
                }
                let (_, buf) = self.rpc_cli.gba_read((sector, SECTOR_SIZE as u16))?;
                if &buf[..] == rom_sector {
                    break;
                }
                println!(
                    "Sector {:08x} check didn't pass, flashing it again...",
                    sector
                );
                println!("=== Expected ===");
                print_hex(
                    &rom[sector as usize..sector as usize + 0x100],
                    sector as u32,
                );
                println!();
                println!("=== Got ===");
                print_hex(&buf[..0x100], sector as u32);
                println!();
                if attempt == 3 {
                    return Err(Error::Generic(format!(
                        "Sector {:08x} check didn't pass after 4 write attempts",
                        sector
                    )));
                }
            }
            pb.add(SECTOR_SIZE as u64);
        }

        progress_bar_finish(&mut pb, "Flashing ROM", rom.len(), start.elapsed());

        // let stats = self.rpc_cli.gb_get_stats(())?.unwrap();
        // info!("{:?}", stats);
        Ok(())
    }

    fn flash_unlock_sector(
        &mut self,
        flash_type: rpc::FlashType,
        sector: u32,
    ) -> Result<(), Error> {
        match flash_type {
            rpc::FlashType::F3 => {
                let w = self.read_word(sector + 2)?;
                println!("a w: {:02x}", (w & 0x00ff) as u8);

                self.rpc_cli.gba_write_word((sector, 0xff))?;
                self.rpc_cli.gba_write_word((sector, 0x60))?;
                self.rpc_cli.gba_write_word((sector, 0xd0))?;
                self.rpc_cli.gba_write_word((sector, 0x90))?;

                info!("Waiting for flash unlock...");
                let w = self.read_word(sector + 2)?;
                println!("b w: {:02x}", (w & 0x00ff) as u8);
                let w = self.read_word(sector + 2)?;
                println!("b w: {:02x}", (w & 0x00ff) as u8);
                let w = self.read_word(sector + 2)?;
                println!("b w: {:02x}", (w & 0x00ff) as u8);
                // while self.read_word(sector + 2)? & 0x03 != 0x00 {}
                Ok(())
            }
        }
    }

    fn flash_erase_sector(&mut self, flash_type: rpc::FlashType, sector: u32) -> Result<(), Error> {
        match flash_type {
            rpc::FlashType::F3 => {
                let w = self.read_word(sector)?;
                println!("a w: {:02x}", (w & 0x00ff) as u8);

                self.rpc_cli.gba_write_word((sector, 0xff))?;
                self.rpc_cli.gba_write_word((sector, 0x20))?;
                self.rpc_cli.gba_write_word((sector, 0xd0))?;

                let w = self.read_word(sector)?;
                println!("a w: {:02x}", (w & 0x00ff) as u8);
                let w = self.read_word(sector)?;
                println!("a w: {:02x}", (w & 0x00ff) as u8);
                let w = self.read_word(sector)?;
                println!("a w: {:02x}", (w & 0x00ff) as u8);
                while self.read_word(sector)? != 0x80 {
                    thread::sleep(Duration::from_millis(100));
                }
                self.rpc_cli.gba_write_word((sector, 0xff))?;
                Ok(())
            }
        }
    }
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
        const SECTOR_SIZE_0: usize = 0x2000;
        const SECTOR_SIZE_1: usize = 0x10000;
        const SECTOR_SIZE_1_ADDR: usize = 0x10000;

        let n = info.rom_banks as usize * ROM_BANK_SIZE as usize;
        let mut pb = new_progress_bar(n as u64);
        let start = Instant::now();
        for bank in 0..info.rom_banks {
            if bank != 0 {
                self.switch_bank_mc(&MemController::Mbc5, &Memory::Rom, bank)?;
            }
            let addr = if bank == 0 { 0x0000 } else { ROM_BANK_SIZE };
            let rom_pos = bank as usize * ROM_BANK_SIZE as usize;
            if let EraseMode::Sector = erase {
                let sector_size = if rom_pos < SECTOR_SIZE_1_ADDR {
                    SECTOR_SIZE_0
                } else {
                    SECTOR_SIZE_1
                };
                let sector_addrs = if sector_size < ROM_BANK_SIZE as usize {
                    (0..ROM_BANK_SIZE as usize / sector_size)
                        .map(|i| (addr + (i * sector_size) as u16, rom_pos + i * sector_size))
                        .collect()
                } else {
                    if rom_pos % sector_size == 0 {
                        vec![(addr, rom_pos)]
                    } else {
                        vec![]
                    }
                };
                for (sector_vaddr, sector_addr) in sector_addrs {
                    let ok = self.rpc_cli.gb_flash_erase_sector(sector_vaddr)?;
                    if !ok {
                        warn!(
                            "gb_flash_erase sector error at address 0x{:04x} (vaddr: 0x{:04x})",
                            sector_addr, sector_vaddr
                        );
                    }
                }
            }
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
            pb.add(ROM_BANK_SIZE as u64);
        }
        progress_bar_finish(&mut pb, "Flashing ROM", n, start.elapsed());

        let stats = self.rpc_cli.gb_get_stats(())?.unwrap();
        info!("{:?}", stats);

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

    let mut rpc_client = rpc::Client::new(device, 0x8100);

    let result = match matches.subcommand() {
        ("debug", Some(app)) => match app.subcommand() {
            ("ping", _) => {
                let ping = [1, 2, 3, 4];
                let pong = rpc_client.ping(ping)?;
                if pong != ping {
                    Err(Error::Generic(format!(
                        "Ping sent {:?}, got {:?}",
                        ping, pong
                    )))
                } else {
                    info!("ping({:?}) -> pong({:?})", ping, pong);
                    Ok(())
                }
            }
            ("gba-test", _) => {
                let mut gba = Gba::new(rpc_client)?;
                gba.test2()
            }
            _ => unreachable!(),
        },
        ("gb", Some(app)) => match (app.subcommand(), &file) {
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
        },
        ("gba", Some(app)) => match (app.subcommand(), &file) {
            (("read-rom", Some(app)), File::Write(path, file)) => {
                let mut gba = Gba::new(rpc_client)?;
                info!("Reading cartridge ROM into {}", path.display());
                let size = app.value_of("size").map(|s| s.parse::<u32>().unwrap());
                gba.read(file, size)
            }
            (("write-rom", Some(app)), File::Read(path, content)) => {
                let diff = match app.value_of("diff") {
                    Some(diff_path) => {
                        let mut content = Vec::new();
                        fs::OpenOptions::new()
                            .read(true)
                            .open(diff_path)?
                            .read_to_end(&mut content)?;
                        Some(content)
                    }
                    None => None,
                };
                let mut gba = Gba::new(rpc_client)?;
                info!("Writing {} into cartridge ROM", path.display());
                gba.flash_write(&content, diff)
            }
            ((cmd, _), _) => Err(Error::Generic(format!("Unexpected subcommand: {}", cmd))),
        },
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
