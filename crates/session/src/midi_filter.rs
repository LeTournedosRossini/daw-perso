//! Filtre d'entrée MIDI d'une piste : une **liste de règles** (appareil, canaux). Un message est accepté s'il correspond à **au moins une**
//! règle ; une liste vide accepte tout (le comportement d'avant). Par exemple « clavier A, canal 1 » et « pad B, tous les canaux » sur la
//! même piste. Réglage de session de la piste (comme son entrée audio) : hors historique, enregistré avec le projet.

/// Tous les canaux (bit `c` = canal `c + 1`).
pub const ALL_CHANNELS: u16 = 0xFFFF;
/// Nombre maximal de règles par piste.
pub const MAX_RULES: usize = 16;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MidiRule {
    /// Nom de l'appareil tel que le système le donne (vide : n'importe lequel).
    pub device: String,
    /// Canaux acceptés, un bit par canal (bit 0 = canal 1).
    pub channels: u16,
}

impl MidiRule {
    pub fn any_device(channels: u16) -> MidiRule {
        MidiRule { device: String::new(), channels }
    }

    fn matches(&self, device: &str, channel: u8) -> bool {
        (self.device.is_empty() || self.device == device) && self.channels & (1u16 << (channel & 15)) != 0
    }
}

/// Un message de l'appareil `device`, sur le canal `channel` (0 à 15), passe-t-il le filtre ? Liste vide : oui.
pub fn accepts(rules: &[MidiRule], device: &str, channel: u8) -> bool {
    rules.is_empty() || rules.iter().any(|r| r.matches(device, channel))
}

/// Vérifie une liste de règles : au plus [`MAX_RULES`], nom d'appareil de 120 caractères au plus, au moins un canal par règle.
pub fn validate(rules: &[MidiRule]) -> Result<(), String> {
    if rules.len() > MAX_RULES {
        return Err(format!("trop de filtres ({MAX_RULES} au plus)"));
    }
    for r in rules {
        if r.device.chars().count() > 120 {
            return Err("nom d'appareil trop long".into());
        }
        if r.channels == 0 {
            return Err("un filtre doit accepter au moins un canal".into());
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_empty_list_accepts_everything_and_otherwise_any_matching_rule_does() {
        assert!(accepts(&[], "Clavier", 3));
        let rules = vec![MidiRule { device: "Clavier A".into(), channels: 1 }, MidiRule { device: "Pad B".into(), channels: ALL_CHANNELS }, MidiRule::any_device(1 << 9)];
        assert!(accepts(&rules, "Clavier A", 0));
        assert!(!accepts(&rules, "Clavier A", 1), "Clavier A : canal 1 seulement");
        assert!(accepts(&rules, "Pad B", 15));
        assert!(!accepts(&rules, "Clavier C", 4));
        assert!(accepts(&rules, "Clavier C", 9), "n'importe quel appareil, canal 10");
    }

    #[test]
    fn rules_are_validated() {
        assert!(validate(&[]).is_ok());
        assert!(validate(&[MidiRule { device: "x".into(), channels: 0 }]).is_err());
        assert!(validate(&vec![MidiRule::any_device(1); MAX_RULES + 1]).is_err());
        assert!(validate(&[MidiRule { device: "é".repeat(121), channels: 1 }]).is_err());
    }
}
