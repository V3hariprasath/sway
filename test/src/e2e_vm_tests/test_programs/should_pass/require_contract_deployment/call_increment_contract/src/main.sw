script;

use increment_abi::Incrementor;

fn main() -> bool {
    let the_abi = abi(Incrementor, 0x15cf47ca82e3156241ed5d2f6071a17c0cbd87738b03cbae8f7c80382a40e48c);
    let _ = the_abi.increment(5);
    let _ = the_abi.increment(5);
    let result = the_abi.get();
    assert(result == 10);
    log(result);

    let call_params = (0x15cf47ca82e3156241ed5d2f6071a17c0cbd87738b03cbae8f7c80382a40e48c, 0, 0);
    let coins = 0;
    let asset_id = 0x0000000000000000000000000000000000000000000000000000000000000000;
    let gas = std::registers::global_gas();
    asm(ra: __addr_of(call_params), rb: coins, rc: __addr_of(asset_id), rd: gas) {
        call ra rb rc rd;
    }

    true
}

fn log(input: u64) {
    asm(r1: input, r2: 42) {
        log r1 r2 r2 r2;
    }
}
