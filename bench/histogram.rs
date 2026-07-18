// benchmark-contract: histogram;seed=123456789;iterations=20000000;bins=64;index=state&63
const ITERATIONS: u64 = 20_000_000;
const SEED: u64 = 123_456_789;

fn finish(value: u64) -> ! {
    #[cfg(aziky_verify)]
    std::io::Write::write_all(&mut std::io::stdout(), &value.to_le_bytes()).unwrap();
    std::process::exit((value & 0x7f) as i32)
}

fn main() {
    let mut state = SEED;
    let mut bins = [0u64; 64];

    let mut i = 0u64;
    while i < ITERATIONS {
        state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223) & 0xffff_ffff;
        let index = (state & 63) as usize;
        bins[index] = bins[index].wrapping_add(1);
        i += 1;
    }

    let mut checksum = state;
    let mut index = 0usize;
    while index < 64 {
        checksum ^= bins[index].wrapping_mul(index as u64 + 1);
        index += 1;
    }
    finish(checksum);
}
