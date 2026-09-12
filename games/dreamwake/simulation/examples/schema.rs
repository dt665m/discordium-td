#[path = "schema/support.rs"]
mod support;
fn main() {
    println!("{}", support::fixture().unwrap());
}
