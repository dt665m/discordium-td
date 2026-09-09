use super::*;

impl MemoryKind {
    pub const ALL: [Self; 6] = [
        Self::Crescent,
        Self::Starfall,
        Self::Nova,
        Self::Blink,
        Self::Aegis,
        Self::Wisp,
    ];
    pub fn name(self) -> &'static str {
        match self {
            Self::Crescent => "Crescent",
            Self::Starfall => "Starfall",
            Self::Nova => "Nova",
            Self::Blink => "Riftstep",
            Self::Aegis => "Aegis",
            Self::Wisp => "Dream Wisp",
        }
    }
    pub fn description(self) -> &'static str {
        match self {
            Self::Crescent => {
                "Sweep a wide crescent in front of you for 48 damage. Knocks enemies away."
            }
            Self::Starfall => "Launch a fast star for 38 damage. Aim through approaching enemies.",
            Self::Nova => "Detonate a 5-unit dream nova around you for 52 damage.",
            Self::Blink => {
                "Blink 6 units in your aim direction, dodging damage and striking the arrival area for 36."
            }
            Self::Aegis => {
                "Gain a 55-point shield for 7 seconds and repel nearby enemies for 22 damage."
            }
            Self::Wisp => {
                "Summon a wisp for 9 seconds. It follows you and shoots nearby enemies for 15 damage."
            }
        }
    }
    pub fn color(self) -> [f32; 3] {
        match self {
            Self::Crescent => [0.35, 0.96, 0.89],
            Self::Starfall => [0.46, 0.65, 1.0],
            Self::Nova => [0.83, 0.43, 1.0],
            Self::Blink => [0.25, 0.9, 1.0],
            Self::Aegis => [1.0, 0.8, 0.34],
            Self::Wisp => [0.55, 1.0, 0.67],
        }
    }
    pub fn cooldown(self) -> f32 {
        match self {
            Self::Crescent => 3.3,
            Self::Starfall => 2.5,
            Self::Nova => 5.5,
            Self::Blink => 4.5,
            Self::Aegis => 7.5,
            Self::Wisp => 10.0,
        }
    }
    pub fn icon(self) -> &'static str {
        match self {
            Self::Crescent => "C",
            Self::Starfall => "S",
            Self::Nova => "N",
            Self::Blink => "R",
            Self::Aegis => "A",
            Self::Wisp => "W",
        }
    }
}
impl EssenceKind {
    pub const ALL: [Self; 6] = [
        Self::Twin,
        Self::Echo,
        Self::Vast,
        Self::Frost,
        Self::Leech,
        Self::Haste,
    ];
    pub fn name(self) -> &'static str {
        match self {
            Self::Twin => "Twin",
            Self::Echo => "Echo",
            Self::Vast => "Vast",
            Self::Frost => "Frost",
            Self::Leech => "Leech",
            Self::Haste => "Haste",
        }
    }
    pub fn description(self) -> &'static str {
        match self {
            Self::Twin => {
                "Strikes and novas repeat after 0.18s at 60% power. Stars split into three; wisps double; Riftstep adds a Nova; Aegis gains 50% shield."
            }
            Self::Echo => {
                "Repeat this Memory after 0.55 seconds at 65% power. Repeat casts cost no recovery."
            }
            Self::Vast => {
                "Increase the area by 55%; stars pierce three enemies, wisps gain range, shields grow stronger."
            }
            Self::Frost => {
                "Memory hits slow enemies by 55% for 2.5 seconds. Slowed targets take 20% more Memory damage."
            }
            Self::Leech => {
                "Heal for 12% of Memory damage dealt. Aegis also instantly restores 18 health."
            }
            Self::Haste => {
                "38% shorter Memory cooldowns. Wisps fire every 0.42s instead of 0.70s; Aegis lasts 10s instead of 7s."
            }
        }
    }
    pub fn color(self) -> [f32; 3] {
        match self {
            Self::Twin => [1.0, 0.71, 0.3],
            Self::Echo => [0.9, 0.46, 1.0],
            Self::Vast => [0.67, 0.64, 1.0],
            Self::Frost => [0.36, 0.86, 1.0],
            Self::Leech => [0.98, 0.39, 0.58],
            Self::Haste => [0.48, 1.0, 0.68],
        }
    }
}
impl EnemyKind {
    pub fn name(self) -> &'static str {
        match self {
            Self::Melee => "Hollow",
            Self::Ranged => "Stargazer",
            Self::Ambusher => "Skitter",
            Self::Support => "Cantor",
            Self::Elite => "Oathbreaker",
            Self::Boss => "The Somnarch",
        }
    }
}
impl UpgradeKind {
    pub const ALL: [Self; 7] = [
        Self::Attack,
        Self::Ability,
        Self::Movement,
        Self::Critical,
        Self::Recovery,
        Self::Health,
        Self::Defense,
    ];
    pub fn name(self) -> &'static str {
        match self {
            Self::Attack => "Keen Edge",
            Self::Ability => "Deep Resonance",
            Self::Movement => "Featherstep",
            Self::Critical => "Perfect Lucidity",
            Self::Recovery => "Quickening",
            Self::Health => "Heart of the Dream",
            Self::Defense => "Moonwoven",
        }
    }
    pub fn description(self) -> &'static str {
        match self {
            Self::Attack => {
                "+30 percentage points basic attack power (100% becomes 130%). Third-strike healing and refresh remain."
            }
            Self::Ability => {
                "+28 percentage points Memory power (100% becomes 128%). Improves damage and shielding."
            }
            Self::Movement => "+14% movement speed and instantly refresh your dash.",
            Self::Critical => {
                "+15 percentage points critical chance. Critical hits deal 190% damage."
            }
            Self::Recovery => {
                "18% shorter cooldowns on all Memories, including those already cooling down."
            }
            Self::Health => "+50 maximum health and restore 75 health now.",
            Self::Defense => "+10 percentage points damage reduction and gain a 35-point shield.",
        }
    }
}
impl Rarity {
    pub fn name(self) -> &'static str {
        match self {
            Self::Common => "Common",
            Self::Rare => "Rare",
            Self::Epic => "Epic",
        }
    }
}
impl MemorySlot {
    pub fn new(kind: MemoryKind) -> Self {
        Self {
            kind,
            level: 1,
            essence: None,
            cooldown: 0.0,
            max_cooldown: kind.cooldown(),
        }
    }
    pub fn ready(&self) -> bool {
        self.cooldown <= 0.0
    }
    pub fn description(&self) -> String {
        let essence = self
            .essence
            .map(|e| format!(" {}: {}", e.name(), e.effect_for(self.kind)))
            .unwrap_or_default();
        format!(
            "{} Rank {} (+{}% power), {:.1}s recovery.{}",
            self.kind.description(),
            self.level,
            self.level.saturating_sub(1) as u32 * 25,
            self.max_cooldown,
            essence
        )
    }
}
impl Reward {
    pub fn name(&self) -> &str {
        &self.title
    }
    pub fn description(&self) -> &str {
        &self.description
    }
    pub fn comparison(&self, slot: &MemorySlot) -> String {
        match self.kind {
            RewardKind::Memory(kind) if kind == slot.kind && slot.level >= 8 => format!(
                "{} is already at maximum rank 8. Choose another slot or gift.",
                kind.name()
            ),
            RewardKind::Memory(kind) if kind == slot.kind => format!(
                "{} rank {} → {}. +25% base power; keep {}.",
                kind.name(),
                slot.level,
                (slot.level + 1).min(8),
                slot.essence
                    .map(|e| e.name())
                    .unwrap_or("an empty Essence socket")
            ),
            RewardKind::Memory(kind) => format!(
                "Replace {} rank {} with {} rank 1. Your {} Essence carries over.",
                slot.kind.name(),
                slot.level,
                kind.name(),
                slot.essence.map(|e| e.name()).unwrap_or("empty")
            ),
            RewardKind::Essence(kind) => format!(
                "{}: {} → {}. {}",
                slot.kind.name(),
                slot.essence.map(|e| e.name()).unwrap_or("No Essence"),
                kind.name(),
                kind.effect_for(slot.kind)
            ),
            RewardKind::Upgrade(kind) => kind.description().into(),
        }
    }
}

impl EssenceKind {
    /// The reward comparison describes what changes on the selected Memory.
    pub fn effect_for(self, memory: MemoryKind) -> &'static str {
        match (self, memory) {
            (Self::Twin, MemoryKind::Crescent) => "Repeat the crescent after 0.18s at 60% power.",
            (Self::Twin, MemoryKind::Starfall) => {
                "Launch three full-power stars in a narrow fan instead of one."
            }
            (Self::Twin, MemoryKind::Nova) => "Repeat the nova after 0.18s at 60% power.",
            (Self::Twin, MemoryKind::Blink) => {
                "After arrival, detonate a 5-unit Nova after 0.18s for 31.2 base damage."
            }
            (Self::Twin, MemoryKind::Aegis) => "Gain 82.5 base shield instead of 55 (+50%).",
            (Self::Twin, MemoryKind::Wisp) => "Summon two full-power wisps instead of one.",
            (Self::Vast, MemoryKind::Crescent) => "Increase crescent reach from 5 to 7.75 units.",
            (Self::Vast, MemoryKind::Starfall) => {
                "Stars grow 55% wider and pierce up to four enemies instead of one."
            }
            (Self::Vast, MemoryKind::Nova) => "Increase nova radius from 5 to 7.75 units.",
            (Self::Vast, MemoryKind::Blink) => {
                "Blink 9.3 units instead of 6. Arrival impact grows from 2.8 to 4.34 units."
            }
            (Self::Vast, MemoryKind::Aegis) => {
                "Gain 85.25 base shield instead of 55; repel radius grows 55%."
            }
            (Self::Vast, MemoryKind::Wisp) => {
                "Wisps target enemies within 18 units instead of 12. Bolts pierce four enemies."
            }
            (Self::Haste, MemoryKind::Wisp) => {
                "38% shorter summon cooldown. Wisps fire every 0.42s instead of 0.70s."
            }
            (Self::Haste, MemoryKind::Aegis) => {
                "38% shorter cooldown. Shield lasts 10s instead of 7s."
            }
            (Self::Haste, _) => "This Memory's cooldown becomes 38% shorter.",
            (Self::Leech, MemoryKind::Aegis) => {
                "Instantly heal 18 base health, plus 12% of damage dealt by the repel pulse."
            }
            (Self::Leech, _) => "Heal for 12% of actual damage dealt by this Memory.",
            (Self::Echo, MemoryKind::Blink) => {
                "Blink again after 0.55s. Second arrival impact deals 65% damage."
            }
            (Self::Echo, MemoryKind::Wisp) => {
                "Summon a second wisp after 0.55s. It lasts 9s and deals 65% damage."
            }
            (Self::Echo, MemoryKind::Aegis) => {
                "Repeat shield and repel after 0.55s at 65% power, with no extra cooldown."
            }
            (Self::Echo, _) => {
                "Repeat this Memory after 0.55s at 65% damage, with no extra cooldown."
            }
            (Self::Frost, _) => {
                "Hits slow enemies by 55% for 2.5s. Already slowed targets take 20% more Memory damage."
            }
        }
    }
}

/// Next-level threshold shared by direct and scheduled engine progression.
pub(crate) fn experience_threshold(level: u32) -> f32 {
    45.0 + level as f32 * 22.0
}
