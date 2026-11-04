//! Test-only stand-in for an agent CLI. See `fm_agents::fake` for the options.

fn main() {
    std::process::exit(fm_agents::fake::main());
}
