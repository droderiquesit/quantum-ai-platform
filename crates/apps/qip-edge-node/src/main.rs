//! The edge cell node. Under construction: the cell composition root in
//! `qip-edge` is being built, and this binary assembles it for one region.

fn main() {
    // Deliberately refuses to pretend it serves until the cell exists. The
    // deployment's exemption test tracks exactly this.
    eprintln!("qip-edge-node: the cell composition root is not yet assembled");
    std::process::exit(78); // EX_CONFIG
}
