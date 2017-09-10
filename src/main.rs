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
use std::num::Wrapping;
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

    // Read Bank 00
    println!("Reading bank 00");
    let addr_start = 0x0000 as u16;
    let addr_end = 0x4000 as u16;
    port.write_all(cmd_read(addr_start, addr_end).as_slice())?;
    port.flush()?;

    let mut buf = vec![0; (addr_end - addr_start) as usize];
    port.read_exact(&mut buf)?;

    println!();
    print_hex(&buf[0x0000..0x0200], 0x0000);
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

    println!("ROM Title: {}", header_info.title);
    println!("Color Gameboy compatibility: {:?}", header_info.cgb);
    println!("License: {:?}", header_info.license);
    println!("Cartridge type:    {:?}", header_info.cart_type);
    println!("Memory controller: {:?}", header_info.mem_controller);
    println!("Rom banks: {}", header_info.rom_banks);
    println!("Ram banks: {}", header_info.ram_banks);
    println!("Ram size:  {} KB", header_info.ram_size);
    println!("Destination: {:?}", header_info.destination);
    println!("Checksum:              {:02x}", header_info.checksum);
    println!("Header checksum check: {:02x}", header_checksum(&buf));
    println!("Global checksum: {:04x}", header_info.global_checksum);


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

    return Ok(());
}

#[derive(Debug, PartialEq)]
enum CGBFlag {
    DMGCompat = 0x80,
    CGBOnly = 0xC0,
}

#[cfg_attr(rustfmt, rustfmt_skip)]
enum_from_primitive! {
#[derive(Debug, PartialEq)]
enum OldLicense {
    None = 0x00, NintendoRD1 = 0x01, Capcom = 0x08,
    ElectronicArts = 0x13, HudsonSoft = 0x18, Bai = 0x19,
    Kss = 0x20, Pow = 0x22, PCMComplete = 0x24,
    SanX = 0x25, KemcoJapan = 0x28, Seta = 0x29,
    Viacom = 0x30, Nintendo = 0x31, Bandai = 0x32,
    OceanAcclaim = 0x33, Konami = 0x34, Hector = 0x35,
    Taito = 0x37, Hudson = 0x38, Banpresto = 0x39,
    UbiSoft = 0x41, Atlus = 0x42, Malibu = 0x44,
    Angel = 0x46, BulletProof = 0x47, Irem = 0x49,
    Absolute = 0x50, Acclaim = 0x51, Activision = 0x52,
    AmericanSammy = 0x53, Konami1 = 0x54, HiTechEntertainment = 0x55,
    LJN = 0x56, Matchbox = 0x57, Mattel = 0x58,
    MiltonBradley = 0x59, Titus = 0x60, Virgin = 0x61,
    LucasArts = 0x64, Ocean = 0x67, ElectronicArts1 = 0x69,
    Infogrames = 0x70, Interplay = 0x71, Broderbund = 0x72,
    Sculptured = 0x73, Sci = 0x75, THQ = 0x78,
    Accolade = 0x79, Misawa = 0x80, Lozc = 0x83,
    TokumaShotenI = 0x86, TsukudaOri = 0x87, Chunsoft = 0x91,
    VideoSystem = 0x92, OceanAcclaim1 = 0x93, Varie = 0x95,
    YonezawasPal = 0x96, Kaneko = 0x97, PackInSoft = 0x99,
    KonamiYuGiOh = 0xA4, Unknown = 0xFF,
}
}

#[derive(Debug, PartialEq)]
enum License {
    NewLicense(char, char),
    OldLicense(OldLicense),
}

enum_from_primitive! {
#[derive(Debug, PartialEq)]
enum Destination {
    Japan = 0x00,
    NonJapan = 0x01,
    Unknown = 0xFF,
}
}

#[cfg_attr(rustfmt, rustfmt_skip)]
enum_from_primitive! {
#[derive(Debug, PartialEq)]
enum CartType {
    RomOnly             = 0x00, Mbc5                       = 0x19,
    Mbc1                = 0x01, Mbc5Ram                    = 0x1A,
    Mbc1Ram             = 0x02, Mbc5RamBattery             = 0x1B,
    Mbc1RamBattery      = 0x03, Mbc5Rumble                 = 0x1C,
    Mbc2                = 0x05, Mbc5RumbleRam              = 0x1D,
    Mbc2Battery         = 0x06, Mbc5RumbleRamBattery       = 0x1E,
    RomRam              = 0x08, Mbc6                       = 0x20,
    RomRamBattery       = 0x09, Mbc7SensorRumbleRamBattery = 0x22,
    Mmm01               = 0x0B,
    Mmm01Ram            = 0x0C,
    Mmm01RamBattery     = 0x0D,
    Mbc3TimerBattery    = 0x0F,
    Mbc3TimerRamBattery = 0x10, PocketCamera               = 0xFC,
    Mbc3                = 0x11, BandaiTama5                = 0xFD,
    Mbc3Ram             = 0x12, HuC3                       = 0xFE,
    Mbc3RamBattery      = 0x13, HuC1RamBattery             = 0xFF,
}
}

enum_from_primitive! {
#[derive(Debug, PartialEq)]
enum RomBanks {
    Banks002 = 0x00,
    Banks004 = 0x01,
    Banks008 = 0x02,
    Banks016 = 0x03,
    Banks032 = 0x04,
    Banks064 = 0x05,
    Banks128 = 0x06,
    Banks256 = 0x07,
    Banks512 = 0x08,
    Banks072 = 0x52,
    Banks080 = 0x53,
    Banks096 = 0x54,
}
}

enum_from_primitive! {
#[derive(Debug, PartialEq)]
enum RamSize {
    KB000 = 0x00,
    KB002 = 0x01,
    KB008 = 0x02,
    KB032 = 0x03,
    KB128 = 0x04,
    KB064 = 0x05,
}
}

#[derive(Debug, PartialEq)]
enum MemController {
    None,
    Mbc1,
    Mbc2,
    Mbc3,
    Mbc5,
    Mbc6,
    Mbc7,
    Mmm01,
    Unknown,
}

#[derive(Debug, PartialEq)]
struct HeaderInfo {
    title: String,
    cgb: Option<CGBFlag>,
    license: License,
    cart_type: CartType,
    mem_controller: MemController,
    rom_banks: usize,
    ram_banks: usize, // in KiB
    ram_size: usize, // in KiB
    destination: Destination,
    checksum: u8,
    global_checksum: u16,
}

#[derive(Debug, PartialEq)]
enum HeaderError {
    UnknownCartType,
    UnknownRomBanks,
    UnknownRamSize,
}

#[cfg_attr(rustfmt, rustfmt_skip)]
fn cart_type_to_mem_controller(cart_type: &CartType) -> MemController {
    return match *cart_type {
        CartType::RomOnly                    => MemController::None,
        CartType::Mbc1                       => MemController::Mbc1,
        CartType::Mbc1Ram                    => MemController::Mbc1,
        CartType::Mbc1RamBattery             => MemController::Mbc1,
        CartType::Mbc2                       => MemController::Mbc2,
        CartType::Mbc2Battery                => MemController::Mbc2,
        CartType::RomRam                     => MemController::None,
        CartType::RomRamBattery              => MemController::None,
        CartType::Mmm01                      => MemController::Mmm01,
        CartType::Mmm01Ram                   => MemController::Mmm01,
        CartType::Mmm01RamBattery            => MemController::Mmm01,
        CartType::Mbc3TimerBattery           => MemController::Mbc3,
        CartType::Mbc3TimerRamBattery        => MemController::Mbc3,
        CartType::Mbc3                       => MemController::Mbc3,
        CartType::Mbc3Ram                    => MemController::Mbc3,
        CartType::Mbc3RamBattery             => MemController::Mbc3,
        CartType::Mbc5                       => MemController::Mbc5,
        CartType::Mbc5Ram                    => MemController::Mbc5,
        CartType::Mbc5RamBattery             => MemController::Mbc5,
        CartType::Mbc5Rumble                 => MemController::Mbc5,
        CartType::Mbc5RumbleRam              => MemController::Mbc5,
        CartType::Mbc5RumbleRamBattery       => MemController::Mbc5,
        CartType::Mbc6                       => MemController::Mbc6,
        CartType::Mbc7SensorRumbleRamBattery => MemController::Mbc7,
        _                                    => MemController::Unknown,
    };
}

#[cfg_attr(rustfmt, rustfmt_skip)]
fn rom_banks_to_usize(rom_banks: RomBanks, mem_controller: &MemController) -> usize {
    return match rom_banks {
        RomBanks::Banks002 => 2,
        RomBanks::Banks004 => 4,
        RomBanks::Banks008 => 8,
        RomBanks::Banks016 => 16,
        RomBanks::Banks032 => 32,
        RomBanks::Banks064 => if *mem_controller == MemController::Mbc1 { 63 } else { 64 },
        RomBanks::Banks128 => if *mem_controller == MemController::Mbc1 { 125 } else { 128 },
        RomBanks::Banks256 => 256,
        RomBanks::Banks512 => 512,
        RomBanks::Banks072 => 52,
        RomBanks::Banks080 => 53,
        RomBanks::Banks096 => 54,
    };
}

#[cfg_attr(rustfmt, rustfmt_skip)]
fn ram_size_to_usize(ram_size: RamSize) -> usize {
    return match ram_size {
        RamSize::KB000 => 0,
        RamSize::KB002 => 2,
        RamSize::KB008 => 8,
        RamSize::KB032 => 32,
        RamSize::KB128 => 128,
        RamSize::KB064 => 64,
    };
}

fn parse_header(bank: &[u8]) -> Result<HeaderInfo, HeaderError> {
    let cgb = if (bank[0x0143] & (CGBFlag::DMGCompat as u8)) != 0x00 {
        Some(CGBFlag::DMGCompat)
    } else if (bank[0x0143] & (CGBFlag::CGBOnly as u8)) != 0x00 {
        Some(CGBFlag::CGBOnly)
    } else {
        None
    };

    let title = if cgb == None {
        String::from_utf8_lossy(&bank[0x0134..0x0143]).to_string()
    } else {
        String::from_utf8_lossy(&bank[0x0134..0x0142]).to_string()
    };

    let cart_type = match CartType::from_u8(bank[0x0147]) {
        Some(t) => t,
        None => return Err(HeaderError::UnknownCartType),
    };

    let mem_controller = cart_type_to_mem_controller(&cart_type);

    let rom_banks = match RomBanks::from_u8(bank[0x0148]) {
        Some(rom_banks) => rom_banks_to_usize(rom_banks, &mem_controller),
        None => return Err(HeaderError::UnknownRomBanks),
    };

    let ram_size = match RamSize::from_u8(bank[0x0149]) {
        Some(ram_size) => ram_size_to_usize(ram_size),
        None => return Err(HeaderError::UnknownRamSize),
    };
    let ram_banks = div_round_up(ram_size, 8);

    let destination = match Destination::from_u8(bank[0x014A]) {
        Some(d) => d,
        None => Destination::Unknown,
    };

    let license = match bank[0x014B] {
        0x33 => License::NewLicense(bank[0x0144] as char, bank[0x0145] as char),
        b => {
            match OldLicense::from_u8(b) {
                Some(ol) => License::OldLicense(ol),
                None => License::OldLicense(OldLicense::Unknown),
            }
        }
    };

    let checksum = bank[0x014D];
    let global_checksum = (bank[0x014E] as u16) << 8 | (bank[0x014F] as u16);

    return Ok(HeaderInfo {
        title: title,
        cgb: cgb,
        license: license,
        cart_type: cart_type,
        mem_controller: mem_controller,
        rom_banks: rom_banks,
        ram_banks: ram_banks,
        ram_size: ram_size,
        destination: destination,
        checksum: checksum,
        global_checksum: global_checksum,
    });
}

fn header_checksum(bank: &[u8]) -> u8 {
    let mut c = Wrapping(0u8);
    for i in 0x0134..0x014D {
        c = c - Wrapping(bank[i]) - Wrapping(1u8);
    }
    return c.0;
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

//#[cfg_attr(rustfmt, rustfmt_skip)]
//static NINTENDO_LOGO: &[u8] = &[
//    0xCE, 0xED, 0x66, 0x66, 0xCC, 0x0D, 0x00, 0x0B, 0x03, 0x73, 0x00, 0x83, 0x00, 0x0C, 0x00, 0x0D,
//    0x00, 0x08, 0x11, 0x1F, 0x88, 0x89, 0x00, 0x0E, 0xDC, 0xCC, 0x6E, 0xE6, 0xDD, 0xDD, 0xD9, 0x99,
//    0xBB, 0xBB, 0x67, 0x63, 0x6E, 0x0E, 0xEC, 0xCC, 0xDD, 0xDC, 0x99, 0x9F, 0xBB, 0xB9, 0x33, 0x3E];
