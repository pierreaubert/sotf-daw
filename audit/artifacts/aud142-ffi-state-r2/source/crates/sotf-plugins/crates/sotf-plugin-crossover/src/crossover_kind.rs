use super::misc::is_linear_phase_type;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum CrossoverKind {
    Lr12,
    Lr24,
    Lr48,
    Butterworth6,
    Butterworth12,
    Butterworth18,
    Butterworth24,
    Butterworth30,
    Butterworth36,
    Butterworth42,
    Butterworth48,
    Bessel12,
    LinearPhase,
}

impl CrossoverKind {
    pub(super) fn as_str(self) -> &'static str {
        match self {
            Self::Lr12 => "LR12",
            Self::Lr24 => "LR24",
            Self::Lr48 => "LR48",
            Self::Butterworth6 => "BW6",
            Self::Butterworth12 => "BW12",
            Self::Butterworth18 => "BW18",
            Self::Butterworth24 => "BW24",
            Self::Butterworth30 => "BW30",
            Self::Butterworth36 => "BW36",
            Self::Butterworth42 => "BW42",
            Self::Butterworth48 => "BW48",
            Self::Bessel12 => "Bessel12",
            Self::LinearPhase => "LinearPhase",
        }
    }

    pub(super) fn is_new_iir(self) -> bool {
        !matches!(self, Self::Lr24 | Self::LinearPhase)
    }

    pub(super) fn is_lr(self) -> bool {
        matches!(self, Self::Lr12 | Self::Lr24 | Self::Lr48)
    }

    pub(super) fn parse(crossover_type: &str) -> Result<Self, String> {
        let normalized = crossover_type.trim();
        if normalized.eq_ignore_ascii_case("lr12") || normalized.eq_ignore_ascii_case("lr2") {
            Ok(Self::Lr12)
        } else if normalized.eq_ignore_ascii_case("lr24") || normalized.eq_ignore_ascii_case("lr4")
        {
            Ok(Self::Lr24)
        } else if normalized.eq_ignore_ascii_case("lr48") || normalized.eq_ignore_ascii_case("lr8")
        {
            Ok(Self::Lr48)
        } else if normalized.eq_ignore_ascii_case("bw6") {
            Ok(Self::Butterworth6)
        } else if normalized.eq_ignore_ascii_case("bw12") {
            Ok(Self::Butterworth12)
        } else if normalized.eq_ignore_ascii_case("bw18") {
            Ok(Self::Butterworth18)
        } else if normalized.eq_ignore_ascii_case("bw24") {
            Ok(Self::Butterworth24)
        } else if normalized.eq_ignore_ascii_case("bw30") {
            Ok(Self::Butterworth30)
        } else if normalized.eq_ignore_ascii_case("bw36") {
            Ok(Self::Butterworth36)
        } else if normalized.eq_ignore_ascii_case("bw42") {
            Ok(Self::Butterworth42)
        } else if normalized.eq_ignore_ascii_case("bw48") {
            Ok(Self::Butterworth48)
        } else if normalized.eq_ignore_ascii_case("bessel12") {
            Ok(Self::Bessel12)
        } else if is_linear_phase_type(normalized) {
            Ok(Self::LinearPhase)
        } else {
            Err(format!(
                "Unsupported crossover type: '{crossover_type}'. Supported: LR12, LR24/LR4, LR48, BW6 through BW48, Bessel12, and LinearPhase/FIR."
            ))
        }
    }
}

/// Return the canonical saved-state family name accepted by the crossover
/// parser, including its legacy spelling and FIR aliases.
pub fn canonical_crossover_type(crossover_type: &str) -> Result<&'static str, String> {
    CrossoverKind::parse(crossover_type).map(CrossoverKind::as_str)
}
