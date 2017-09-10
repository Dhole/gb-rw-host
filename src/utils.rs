pub fn print_hex(buf: &[u8], addr_start: u16) {
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

pub fn div_round_up(x: usize, y: usize) -> usize {
    return (x + y - 1) / y;
}
