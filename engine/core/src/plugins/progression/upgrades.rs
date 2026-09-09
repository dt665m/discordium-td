pub fn spend_currency(balance: &mut u32, cost: u32) -> bool {
    if *balance < cost {
        return false;
    }
    *balance -= cost;
    true
}
/// Validate both constraints before mutating either currency or rank.
pub fn purchase_rank(balance: &mut u32, rank: &mut u8, cost: u32, cap: u8) -> bool {
    if *rank >= cap || *balance < cost {
        return false;
    }
    *balance -= cost;
    *rank += 1;
    true
}
pub fn add_capped(value: f32, amount: f32, cap: f32) -> f32 {
    (value + amount).min(cap)
}
pub fn multiply_capped(value: f32, factor: f32, cap: f32) -> f32 {
    (value * factor).min(cap)
}
pub fn multiply_floored(value: f32, factor: f32, floor: f32) -> f32 {
    (value * factor).max(floor)
}
