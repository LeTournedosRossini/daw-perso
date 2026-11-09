//! Chaîne latérale d'un effet : réglage hors historique, renumérotation des pistes, retrait et annulation, refus.

use std::sync::Arc;

use daw_core_types::NodeId;
use daw_engine::{GraphDesc, Insert, InsertFactory, TrackFormat};
use daw_session::Session;

struct Nop;
impl Insert for Nop {
    fn latency(&self) -> u32 {
        0
    }
    fn process(&mut self, in_l: &[f32], in_r: &[f32], out_l: &mut [f32], out_r: &mut [f32]) {
        out_l.copy_from_slice(in_l);
        out_r.copy_from_slice(in_r);
    }
    fn set_param(&mut self, _: u32, _: f64) {}
    fn reset(&mut self) {}
}

fn nop() -> InsertFactory {
    Arc::new(|_, _, _| Ok(Box::new(Nop) as Box<dyn Insert>))
}

fn setup() -> (Session, NodeId, NodeId, NodeId, u32) {
    let mut g = GraphDesc::new();
    let first = g.add_track("première", TrackFormat::Stereo);
    let kick = g.add_track("grosse caisse", TrackFormat::Stereo);
    let bass = g.add_track("basse", TrackFormat::Stereo);
    let mut s = Session::new(g);
    let fx = s.add_effect(bass, "compresseur", nop()).unwrap();
    (s, first, kick, bass, fx)
}

#[test]
fn a_side_chain_is_set_without_history_and_cleared() {
    let (mut s, _, kick, _, fx) = setup();
    let label = s.undo_label().map(str::to_owned);
    assert_eq!(s.effect_sidechain(fx), None);
    s.set_effect_sidechain_untracked(fx, Some(kick)).unwrap();
    assert_eq!(s.effect_sidechain(fx), Some(kick));
    assert_eq!(s.undo_label().map(str::to_owned), label, "hors historique");
    s.set_effect_sidechain_untracked(fx, None).unwrap();
    assert_eq!(s.effect_sidechain(fx), None);
}

#[test]
fn invalid_side_chains_are_refused_and_change_nothing() {
    let (mut s, _, kick, bass, fx) = setup();
    for (src, why) in [(NodeId::MASTER, "la sortie principale"), (bass, "le nœud lui-même"), (NodeId(99), "un nœud inconnu")] {
        assert!(s.set_effect_sidechain_untracked(fx, Some(src)).is_err(), "{why}");
        assert_eq!(s.effect_sidechain(fx), None, "{why}");
    }
    assert!(s.set_effect_sidechain_untracked(777, Some(kick)).is_err(), "effet inconnu");
    // Une boucle : la grosse caisse recevrait la chaîne latérale de la basse qui reçoit celle de la grosse caisse.
    s.set_effect_sidechain_untracked(fx, Some(kick)).unwrap();
    let fx2 = s.add_effect(kick, "gate", nop()).unwrap();
    assert!(s.set_effect_sidechain_untracked(fx2, Some(bass)).is_err());
    assert_eq!(s.effect_sidechain(fx2), None);
}

#[test]
fn the_source_follows_renumbering_and_comes_back_with_a_deleted_track() {
    let (mut s, first, kick, bass, fx) = setup();
    s.set_effect_sidechain_untracked(fx, Some(kick)).unwrap();
    // Retirer la première piste décale tout : la grosse caisse est maintenant le nœud 1, la basse le 2.
    s.remove_node(first, &mut |_| panic!("pas d'effet")).unwrap();
    assert_eq!(s.effect_sidechain(fx), Some(NodeId(1)));
    // Retirer la source : la chaîne latérale disparaît ; annuler la rend.
    s.remove_node(NodeId(1), &mut |_| panic!("pas d'effet")).unwrap();
    assert_eq!(s.effect_sidechain(fx), None);
    s.undo();
    assert_eq!(s.effect_sidechain(fx), Some(NodeId(1)));
    s.undo();
    assert_eq!(s.effect_sidechain(fx), Some(kick));
    assert_eq!(s.locate_insert(fx).map(|(n, _)| n), Some(bass.0 as usize));
}

#[test]
fn removing_the_effect_and_restoring_it_keeps_its_side_chain() {
    let (mut s, _, kick, _, fx) = setup();
    s.set_effect_sidechain_untracked(fx, Some(kick)).unwrap();
    s.remove_effect(fx, &mut |_| nop()).unwrap();
    assert_eq!(s.effect_sidechain(fx), None);
    s.undo();
    assert_eq!(s.effect_sidechain(fx), Some(kick));
}
