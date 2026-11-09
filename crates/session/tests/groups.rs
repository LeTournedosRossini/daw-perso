//! Groupes de pistes du projet, coupe / solo des envois : annulation, renumérotation, projet.

use std::sync::Arc;

use daw_core_types::NodeId;
use daw_engine::{AudioData, GraphDesc, TrackFormat};
use daw_session::groups::Group;
use daw_session::project::AudioLoader;
use daw_session::Session;

struct NoAudio;
impl AudioLoader for NoAudio {
    fn load(&self, _: &str) -> Result<Arc<AudioData>, String> {
        Err("pas d'audio".into())
    }
}

fn setup() -> (Session, NodeId, NodeId, NodeId, NodeId) {
    let mut g = GraphDesc::new();
    let kick = g.add_track("Grosse caisse", TrackFormat::Mono);
    let snare = g.add_track("Caisse claire", TrackFormat::Mono);
    let bass = g.add_track("Basse", TrackFormat::Mono);
    let ret = g.add_bus("Retour");
    (Session::new(g), kick, snare, bass, ret)
}

fn group(id: u32, name: &str) -> Group {
    Group { id, name: name.into(), color: "#ff8800".into() }
}

#[test]
fn a_group_gathers_nodes_and_is_one_undoable_step() {
    let (mut s, kick, snare, bass, _) = setup();
    let id = s.alloc_group_id();
    s.set_groups("Créer un groupe", vec![group(id, "Batterie")], &[(kick, id), (snare, id)]).unwrap();
    assert_eq!(s.groups().len(), 1);
    assert_eq!(s.group_members(id), vec![kick, snare]);
    assert_eq!(s.node_group(bass), 0);
    assert_eq!(s.undo_label(), Some("Créer un groupe"));
    s.undo();
    assert!(s.groups().is_empty());
    assert_eq!(s.node_group(kick), 0);
    s.redo();
    assert_eq!(s.group_members(id), vec![kick, snare]);
}

#[test]
fn invalid_groups_are_refused_without_trace() {
    let (mut s, kick, snare, _, _) = setup();
    let depth = s.undo_label().map(str::to_owned);
    assert!(s.set_groups("x", vec![group(1, " ")], &[]).is_err(), "nom vide");
    assert!(s.set_groups("x", vec![group(1, &"a".repeat(41))], &[]).is_err(), "nom trop long");
    assert!(s.set_groups("x", vec![Group { id: 1, name: "a".into(), color: "rouge".into() }], &[]).is_err(), "couleur");
    assert!(s.set_groups("x", vec![group(0, "a")], &[]).is_err(), "identifiant nul");
    assert!(s.set_groups("x", vec![group(1, "a"), group(1, "b")], &[]).is_err(), "identifiant en double");
    assert!(s.set_groups("x", vec![group(1, "a")], &[(kick, 2)]).is_err(), "groupe inconnu");
    assert!(s.set_groups("x", vec![group(1, "a")], &[(NodeId::MASTER, 1)]).is_err(), "sortie principale");
    assert!(s.set_groups("x", vec![group(1, "a")], &[(NodeId(99), 1)]).is_err(), "membre inexistant");
    assert!(s.set_groups("x", vec![group(1, "a"), group(2, "b")], &[(snare, 1), (snare, 2)]).is_err(), "deux groupes pour un nœud");
    assert!(s.set_groups("x", (1..=65).map(|i| group(i, "g")).collect(), &[]).is_err(), "trop de groupes");
    assert_eq!(s.undo_label().map(str::to_owned), depth);
    assert!(s.groups().is_empty());
}

#[test]
fn membership_follows_renumbering_and_comes_back_with_a_deleted_track() {
    let (mut s, kick, snare, bass, _) = setup();
    s.set_groups("g", vec![group(1, "Rythmique")], &[(snare, 1), (bass, 1)]).unwrap();
    // Retirer la grosse caisse décale les numéros : les membres suivent.
    s.remove_node(kick, &mut |_| panic!("pas d'effet")).unwrap();
    assert_eq!(s.group_members(1), vec![NodeId(1), NodeId(2)], "caisse claire et basse ont reculé d'un cran");
    // Retirer un membre puis annuler : il revient dans son groupe.
    s.remove_node(NodeId(1), &mut |_| panic!("pas d'effet")).unwrap();
    assert_eq!(s.group_members(1), vec![NodeId(1)]);
    s.undo();
    assert_eq!(s.group_members(1), vec![NodeId(1), NodeId(2)]);
    s.undo();
    assert_eq!(s.group_members(1), vec![snare, bass]);
}

#[test]
fn send_mute_and_solo_are_set_without_history_and_only_for_an_existing_send() {
    let (mut s, kick, snare, _, ret) = setup();
    s.add_send(kick, ret).unwrap();
    s.add_send(snare, ret).unwrap();
    let label = s.undo_label().map(str::to_owned);
    assert!(s.set_send_flags_untracked(kick, ret, Some(true), None));
    assert!(s.set_send_flags_untracked(snare, ret, None, Some(true)));
    assert!(!s.set_send_flags_untracked(kick, NodeId(77), Some(true), None));
    assert_eq!(s.undo_label().map(str::to_owned), label, "hors historique");
    let st = |n: NodeId| s.graph().nodes[n.0 as usize].sends[0].clone();
    assert!(st(kick).mute && !st(kick).solo);
    assert!(st(snare).solo && !st(snare).mute);
}

#[test]
fn groups_and_send_flags_survive_a_save_and_reload() {
    let (mut s, kick, snare, bass, ret) = setup();
    s.add_send(kick, ret).unwrap();
    s.set_send_flags_untracked(kick, ret, Some(true), Some(false));
    s.set_groups("g", vec![group(3, "Batterie"), group(5, "Cordes")], &[(kick, 3), (snare, 3), (bass, 5)]).unwrap();
    let (json, _) = s.to_json(48_000).unwrap();
    let back = Session::from_json(&json, &NoAudio).unwrap().0;
    assert_eq!(back.groups(), s.groups());
    assert_eq!(back.group_members(3), vec![kick, snare]);
    assert_eq!(back.group_members(5), vec![bass]);
    assert!(back.graph().nodes[kick.0 as usize].sends[0].mute);
    assert!(back.undo_label().is_none(), "un projet ouvert n'a pas d'historique");
    assert_eq!(back.to_json(48_000).unwrap().0, json, "réécriture identique");
    // Un identifiant neuf ne reprend pas un ancien.
    let mut back = back;
    assert!(back.alloc_group_id() > 5);
}

#[test]
fn an_older_project_without_groups_opens_with_none() {
    let (s, ..) = setup();
    let (json, _) = s.to_json(48_000).unwrap();
    let old = json.replace("\"version\": 19", "\"version\": 14");
    let back = Session::from_json(&old, &NoAudio).unwrap().0;
    assert!(back.groups().is_empty());
}

fn with_return() -> (Session, NodeId, NodeId, NodeId, NodeId) {
    let (mut s, kick, snare, bass, ret) = setup();
    s.set_node_phys(ret, Some(daw_engine::PhysOut { first: 3, stereo: true, mono: false })).unwrap();
    (s, kick, snare, bass, ret)
}

#[test]
fn a_return_with_its_own_groups_ignores_the_project_ones_and_going_back_restores_them() {
    let (mut s, kick, snare, bass, ret) = with_return();
    s.set_groups("g", vec![group(1, "Batterie")], &[(kick, 1), (snare, 1)]).unwrap();
    assert_eq!(s.effective_group_of(kick, Some(ret)), 1, "sans groupes propres : ceux du projet");
    s.set_return_groups("Groupes du retour", ret, Some((vec![group(2, "Tout sauf basse")], vec![(kick, 2), (snare, 2)]))).unwrap();
    assert_eq!(s.undo_label(), Some("Groupes du retour"));
    assert_eq!(s.effective_group_list(Some(ret))[0].name, "Tout sauf basse");
    assert_eq!(s.effective_members(2, Some(ret)), vec![kick, snare]);
    assert_eq!(s.effective_group_of(kick, Some(ret)), 2);
    assert_eq!(s.effective_group_of(bass, Some(ret)), 0);
    assert_eq!(s.effective_group_of(kick, None), 1, "le projet n'a pas bougé");
    assert_eq!(s.groups().len(), 1);
    s.set_return_groups("Retour aux groupes du projet", ret, None).unwrap();
    assert_eq!(s.effective_group_of(kick, Some(ret)), 1);
    s.undo();
    assert_eq!(s.effective_group_of(kick, Some(ret)), 2);
    s.undo();
    assert!(s.return_groups(ret).is_none());
}

#[test]
fn return_groups_are_refused_unless_for_a_return_with_real_sources() {
    let (mut s, kick, snare, bass, ret) = with_return();
    let depth = s.undo_label().map(str::to_owned);
    assert!(s.set_return_groups("x", kick, Some((vec![group(1, "a")], vec![]))).is_err(), "pas un retour");
    assert!(s.set_return_groups("x", NodeId::MASTER, Some((vec![group(1, "a")], vec![]))).is_err());
    assert!(s.set_return_groups("x", ret, Some((vec![group(1, "a")], vec![(ret, 1)]))).is_err(), "le retour lui-même");
    assert!(s.set_return_groups("x", ret, Some((vec![group(1, "a")], vec![(snare, 2)]))).is_err(), "groupe inconnu");
    assert!(s.set_return_groups("x", ret, Some((vec![group(1, " ")], vec![]))).is_err(), "nom vide");
    assert!(s.set_return_groups("x", ret, Some((vec![group(1, "a"), group(2, "b")], vec![(bass, 1), (bass, 2)]))).is_err(), "deux groupes");
    assert_eq!(s.undo_label().map(str::to_owned), depth);
}

#[test]
fn return_groups_follow_renumbering_and_come_back_with_a_deleted_track_or_return() {
    let (mut s, kick, snare, bass, ret) = with_return();
    s.set_return_groups("r", ret, Some((vec![group(7, "Cordes")], vec![(snare, 7), (bass, 7)]))).unwrap();
    // Retirer la grosse caisse décale tout : le retour est maintenant le nœud 3 et ses membres 1 et 2.
    s.remove_node(kick, &mut |_| panic!("pas d'effet")).unwrap();
    let ret_now = NodeId(3);
    assert_eq!(s.effective_members(7, Some(ret_now)), vec![NodeId(1), NodeId(2)]);
    // Retirer un membre puis annuler : il revient dans le groupe du retour.
    s.remove_node(NodeId(1), &mut |_| panic!("pas d'effet")).unwrap();
    assert_eq!(s.effective_members(7, Some(NodeId(2))), vec![NodeId(1)]);
    s.undo();
    assert_eq!(s.effective_members(7, Some(ret_now)), vec![NodeId(1), NodeId(2)]);
    // Retirer le retour lui-même puis annuler : il revient avec ses groupes.
    s.remove_node(ret_now, &mut |_| panic!("pas d'effet")).unwrap();
    s.undo();
    assert_eq!(s.effective_members(7, Some(ret_now)), vec![NodeId(1), NodeId(2)]);
    s.undo();
    assert_eq!(s.effective_members(7, Some(ret)), vec![snare, bass]);
    assert_eq!(s.effective_group_list(Some(ret))[0].name, "Cordes");
}

#[test]
fn return_groups_survive_a_save_and_reload() {
    let (mut s, kick, snare, bass, ret) = with_return();
    s.set_groups("g", vec![group(1, "Projet")], &[(kick, 1)]).unwrap();
    s.set_return_groups("r", ret, Some((vec![group(9, "Du retour")], vec![(snare, 9), (bass, 9)]))).unwrap();
    let (json, _) = s.to_json(48_000).unwrap();
    let mut back = Session::from_json(&json, &NoAudio).unwrap().0;
    assert_eq!(back.effective_members(9, Some(ret)), vec![snare, bass]);
    assert_eq!(back.effective_group_of(kick, None), 1);
    assert_eq!(back.to_json(48_000).unwrap().0, json, "réécriture identique");
    assert!(back.alloc_group_id() > 9, "un identifiant neuf ne reprend pas celui d'un groupe de retour");
}

#[test]
fn a_return_address_token_follows_its_node_and_is_saved_with_the_project() {
    let (mut s, kick, _snare, _bass, ret) = with_return();
    assert_eq!(s.monitor_token(ret), "");
    assert!(s.set_monitor_token_untracked(ret, "abc123"));
    assert!(!s.set_monitor_token_untracked(NodeId::MASTER, "x"), "la sortie principale n'a pas d'adresse");
    assert!(!s.set_monitor_token_untracked(NodeId(99), "x"));
    let label = s.undo_label().map(str::to_owned);
    // Retirer une piste avant le retour le décale : son jeton le suit ; annuler le rend à sa place.
    s.remove_node(kick, &mut |_| panic!("pas d'effet")).unwrap();
    assert_eq!(s.monitor_token(NodeId(3)), "abc123");
    assert_eq!(s.monitor_token(NodeId(1)), "");
    // Retirer le retour lui-même puis annuler : le jeton revient avec lui.
    s.remove_node(NodeId(3), &mut |_| panic!("pas d'effet")).unwrap();
    s.undo();
    assert_eq!(s.monitor_token(NodeId(3)), "abc123");
    s.undo();
    assert_eq!(s.monitor_token(ret), "abc123");
    assert_eq!(s.undo_label().map(str::to_owned), label, "le jeton n'est pas dans l'historique");
    let (json, _) = s.to_json(48_000).unwrap();
    let back = Session::from_json(&json, &NoAudio).unwrap().0;
    assert_eq!(back.monitor_token(ret), "abc123");
    assert_eq!(back.to_json(48_000).unwrap().0, json);
}
