use std::sync::Arc;

use daw_core_types::time::Frac;
use daw_core_types::NodeId;
use daw_engine::{AudioData, Clip, GraphDesc, InsertFactory, NodeParams, TrackFormat};
use daw_offline_render::{render, RenderSpec};
use daw_session::nodes::NewNode;
use daw_session::{MClip, Session, SessionError};

const SR: u32 = 48_000;

fn noise(seed: u32, len: usize) -> Vec<f32> {
    let mut s = seed;
    (0..len)
        .map(|_| {
            s = s.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            (s >> 8) as f32 / 16_777_216.0 - 0.5
        })
        .collect()
}

fn clip(seed: u32) -> Clip {
    Clip::whole(AudioData::new(SR, vec![noise(seed, 3_000), noise(seed + 1, 3_000)]).unwrap(), 100 * seed as u64)
}

fn keep(_: &daw_engine::InsertDesc) -> InsertFactory {
    Arc::new(|_, _, _| Err("test".into()))
}

/// Projet : pistes A, B, C et bus X (C passe par X) ; une piste a un envoi vers X.
fn project() -> (Session, [NodeId; 5]) {
    let mut g = GraphDesc::new();
    let a = g.add_track("A", TrackFormat::Stereo);
    let b = g.add_track("B", TrackFormat::Mono);
    let c = g.add_track("C", TrackFormat::Stereo);
    let x = g.add_bus("X");
    g.set_output(c, x);
    g.add_send(a, x, 0.5);
    g.add_clip(a, clip(1));
    g.add_clip(b, clip(2));
    g.add_clip(c, clip(3));
    let mut s = Session::new(g);
    s.set_params_untracked(b, NodeParams { gain: 0.5, pan: 0.2, ..Default::default() }).unwrap();
    s.set_node_color_untracked(a, "#111111");
    s.set_node_color_untracked(b, "#222222");
    s.set_node_color_untracked(c, "#333333");
    s.set_node_color_untracked(x, "#444444");
    (s, [NodeId(0), a, b, c, x])
}

fn hash(s: &Session) -> u64 {
    render(&RenderSpec::new(SR, 4_000, s.graph().clone())).unwrap().hash()
}

/// Résumé du graphe : nom, numéro, sortie, envois, nombre de clips, couleur.
fn shape(s: &Session) -> Vec<(String, u32, Option<u32>, Vec<(u32, u32)>, usize, String)> {
    s.graph().nodes.iter().map(|n| (n.name.clone(), n.id.0, n.output.map(|o| o.0), n.sends.iter().map(|x| (x.to.0, x.gain.to_bits())).collect(), n.clips.len(), s.node_color(n.id).to_owned())).collect()
}

#[test]
fn adding_a_track_or_a_bus_appends_it_and_is_one_undo_step() {
    let (mut s, _) = project();
    let before = shape(&s);
    let t = s.add_node(NewNode::Stereo, "  Piste 4  ", "#abcdef").unwrap();
    let b = s.add_node(NewNode::Bus, "Bus 2", "#123456").unwrap();
    assert_eq!((t.0, b.0), (5, 6));
    assert_eq!(s.graph().nodes[5].name, "Piste 4", "nom nettoyé");
    assert_eq!((s.graph().nodes[5].output, s.graph().nodes[6].output), (Some(NodeId(0)), Some(NodeId(0))));
    assert_eq!(s.node_color(t), "#abcdef");
    assert_eq!(s.display_order().last(), Some(&b));
    assert_eq!(s.undo_label(), Some("Ajouter une piste"));
    s.undo();
    s.undo();
    assert_eq!(shape(&s), before);
    assert!(s.redo() && s.redo());
    assert_eq!(s.graph().nodes.len(), 7);
    assert_eq!(s.add_node(NewNode::Mono, "   ", "").unwrap_err(), SessionError::BadName);
    assert_eq!(s.add_node(NewNode::Mono, &"x".repeat(61), "").unwrap_err(), SessionError::BadName);
}

#[test]
fn removing_a_track_renumbers_everything_and_undo_puts_it_back_exactly() {
    let (mut s, [_, a, b, c, x]) = project();
    let before = (shape(&s), hash(&s));
    let version = s.structure_version();
    s.remove_node(b, &mut keep).unwrap();
    assert!(s.structure_version() > version);
    // Les suivants sont renumérotés : C devient 2, X devient 3, et la sortie de C suit X.
    let names: Vec<_> = s.graph().nodes.iter().map(|n| (n.name.as_str(), n.id.0)).collect();
    assert_eq!(names, [("Master", 0), ("A", 1), ("C", 2), ("X", 3)]);
    assert_eq!(s.graph().nodes[2].output, Some(NodeId(3)), "C passe toujours par X");
    assert_eq!(s.graph().nodes[1].sends[0].to, NodeId(3), "l'envoi de A suit X");
    assert_eq!(s.node_color(NodeId(2)), "#333333", "les couleurs suivent");
    assert_eq!(s.display_order(), &[NodeId(1), NodeId(2), NodeId(3)]);
    // Le rendu est celui d'un projet construit sans B.
    let mut g = GraphDesc::new();
    let a2 = g.add_track("A", TrackFormat::Stereo);
    let c2 = g.add_track("C", TrackFormat::Stereo);
    let x2 = g.add_bus("X");
    g.set_output(c2, x2);
    g.add_send(a2, x2, 0.5);
    g.add_clip(a2, clip(1));
    g.add_clip(c2, clip(3));
    assert_eq!(hash(&s), render(&RenderSpec::new(SR, 4_000, g)).unwrap().hash());
    // Annuler : tout est revenu, numéros, sorties, clips, couleurs, rendu.
    s.undo();
    assert_eq!((shape(&s), hash(&s)), before);
    assert_eq!(s.display_order(), &[a, b, c, x]);
    assert_eq!(s.graph().nodes[2].params.gain, 0.5, "réglages de la piste revenus");
    // Rétablir puis annuler encore : stable.
    assert!(s.redo());
    assert_eq!(s.graph().nodes.len(), 4);
    s.undo();
    assert_eq!((shape(&s), hash(&s)), before);
}

#[test]
fn undoing_a_deletion_keeps_mixer_settings_changed_in_between() {
    let (mut s, [_, a, b, ..]) = project();
    s.remove_node(b, &mut keep).unwrap();
    // Réglage (hors historique) d'une autre piste après la suppression : il ne doit pas être perdu à l'annulation.
    s.set_params_untracked(NodeId(1), NodeParams { gain: 0.25, pan: -0.5, mute: true, ..Default::default() }).unwrap();
    s.undo();
    let p = &s.graph().nodes[a.0 as usize].params;
    assert_eq!((p.gain, p.pan, p.mute), (0.25, -0.5, true));
    assert_eq!(s.graph().nodes[b.0 as usize].params.gain, 0.5);
}

#[test]
fn removing_a_bus_sends_its_tracks_to_the_master_and_undo_reconnects_them() {
    let (mut s, [_, a, _, c, x]) = project();
    let before = (shape(&s), hash(&s));
    s.remove_node(x, &mut keep).unwrap();
    assert_eq!(s.graph().nodes[c.0 as usize].output, Some(NodeId(0)), "C repart vers la sortie principale");
    assert!(s.graph().nodes[a.0 as usize].sends.is_empty(), "les envois vers le bus disparaissent");
    s.graph().validate().unwrap();
    s.undo();
    assert_eq!((shape(&s), hash(&s)), before);
    assert_eq!(s.graph().nodes[c.0 as usize].output, Some(x));
}

#[test]
fn clips_midi_and_automation_follow_their_track_through_renumbering() {
    let (mut s, [_, a, b, c, _]) = project();
    let mc = s.add_midi_clip(c, MClip::new(Frac::ZERO, Frac::new(1, 1).unwrap())).unwrap();
    let clip_c = s.clip_ids(c)[0];
    s.remove_node(a, &mut keep).unwrap();
    // C est maintenant le nœud 2 ; ses clips gardent leurs identifiants.
    assert_eq!(s.clip_ids(NodeId(2))[0], clip_c);
    assert_eq!(s.midi_clip_ids(NodeId(2)), vec![mc]);
    assert!(s.midi_clip(mc).is_some());
    // L'édition d'un clip continue de marcher sur le nœud renuméroté.
    s.move_clip(clip_c, 1_000).unwrap();
    s.undo();
    s.undo();
    assert_eq!(s.clip_ids(c)[0], clip_c);
    assert_eq!(s.midi_clip_ids(c), vec![mc]);
    let _ = b;
}

#[test]
fn routing_is_checked_and_each_change_is_undoable() {
    let (mut s, [_, a, _, c, x]) = project();
    let y = s.add_node(NewNode::Bus, "Y", "").unwrap();
    // Un bus peut aller vers un autre bus ; la boucle est refusée.
    s.set_node_output(x, y).unwrap();
    assert!(matches!(s.set_node_output(y, x), Err(SessionError::Routing(_))), "boucle X → Y → X");
    assert!(matches!(s.set_node_output(a, c), Err(SessionError::Routing(_))), "une piste n'est pas une destination");
    assert!(matches!(s.set_node_output(y, y), Err(SessionError::Routing(_))));
    assert_eq!(s.set_node_output(NodeId(0), x).unwrap_err(), SessionError::MasterNode);
    s.set_node_output(a, x).unwrap();
    assert_eq!(s.graph().nodes[a.0 as usize].output, Some(x));
    s.undo();
    assert_eq!(s.graph().nodes[a.0 as usize].output, Some(NodeId(0)));
    s.undo();
    assert_eq!(s.graph().nodes[x.0 as usize].output, Some(NodeId(0)));
    // Rien n'est inscrit pour une demande refusée ni pour une redirection vers la sortie actuelle.
    let depth = s.undo_label().map(str::to_owned);
    s.set_node_output(a, NodeId(0)).unwrap();
    assert_eq!(s.undo_label().map(str::to_owned), depth);
}

#[test]
fn rename_color_and_order_are_undoable_and_validated() {
    let (mut s, [_, a, b, c, x]) = project();
    s.rename_node(b, "Basse").unwrap();
    s.set_node_color(b, "#00ff00").unwrap();
    s.move_node(c, Some(a)).unwrap();
    assert_eq!(s.display_order(), &[c, a, b, x]);
    s.move_node(c, None).unwrap();
    assert_eq!(s.display_order(), &[a, b, x, c]);
    s.move_node(a, Some(x)).unwrap();
    assert_eq!(s.display_order(), &[b, a, x, c]);
    assert!(matches!(s.move_node(a, Some(NodeId(42))), Err(SessionError::UnknownNode(_))));
    for _ in 0..3 {
        s.undo();
    }
    s.undo();
    s.undo();
    assert_eq!(s.graph().nodes[b.0 as usize].name, "B");
    assert_eq!(s.node_color(b), "#222222");
    assert_eq!(s.display_order(), &[a, b, c, x]);
    assert_eq!(s.rename_node(NodeId(0), "Autre").unwrap_err(), SessionError::MasterNode);
    assert_eq!(s.rename_node(b, "").unwrap_err(), SessionError::BadName);
    assert_eq!(s.remove_node(NodeId(0), &mut keep).unwrap_err(), SessionError::MasterNode);
    assert_eq!(s.remove_node(NodeId(99), &mut keep).unwrap_err(), SessionError::UnknownNode(NodeId(99)));
    assert_eq!(s.move_node(NodeId(0), None).unwrap_err(), SessionError::MasterNode);
}

#[test]
fn deleting_keeps_the_display_order_and_undo_restores_the_position() {
    let (mut s, [_, a, b, c, x]) = project();
    s.move_node(x, Some(a)).unwrap(); // X, A, B, C
    s.move_node(b, Some(x)).unwrap(); // B, X, A, C
    assert_eq!(s.display_order(), &[b, x, a, c]);
    s.remove_node(a, &mut keep).unwrap();
    // A (1) disparaît : B (2) devient 1, C (3) devient 2, X (4) devient 3 ; l'ordre garde la même suite.
    assert_eq!(s.display_order(), &[NodeId(1), NodeId(3), NodeId(2)]);
    s.undo();
    assert_eq!(s.display_order(), &[b, x, a, c]);
}

#[test]
fn removed_effects_get_the_factory_the_caller_chose_back_on_undo() {
    let (mut s, [_, a, ..]) = project();
    let original: InsertFactory = Arc::new(|_, _, _| Err("d'origine".into()));
    let id = s.add_insert(a, "Plugin", Arc::clone(&original)).unwrap();
    let replacement: InsertFactory = Arc::new(|_, _, _| Err("avec état".into()));
    let mut seen = Vec::new();
    s.remove_node(a, &mut |ins: &daw_engine::InsertDesc| {
        seen.push(ins.id);
        Arc::clone(&replacement)
    })
    .unwrap();
    assert_eq!(seen, vec![id]);
    s.undo();
    let back = &s.graph().nodes[a.0 as usize].inserts[0];
    assert_eq!(back.id, id, "même identifiant d'effet");
    assert!(Arc::ptr_eq(&back.factory, &replacement), "l'usine choisie par l'appelant");
}

#[test]
fn display_order_and_colors_can_be_set_without_history_but_only_as_a_permutation() {
    let (mut s, [_, a, b, c, x]) = project();
    s.set_display_order_untracked(&[x, c, b, a]);
    assert_eq!(s.display_order(), &[x, c, b, a]);
    s.set_display_order_untracked(&[x, c, b]); // pas une permutation : ignoré
    s.set_display_order_untracked(&[x, c, b, b]);
    assert_eq!(s.display_order(), &[x, c, b, a]);
    assert!(s.undo_label().is_none(), "aucune entrée d'historique");
}

#[test]
fn project_file_round_trip_keeps_the_new_structure() {
    // La structure (nœuds, sorties, envois) est celle que `to_json` écrit : ajouter un bus puis rediriger survit à l'aller-retour.
    let (mut s, [_, a, ..]) = project();
    let y = s.add_node(NewNode::Bus, "Y", "").unwrap();
    s.set_node_output(a, y).unwrap();
    s.register_asset("a.wav", s.graph().nodes[1].clips[0].data.clone());
    s.register_asset("b.wav", s.graph().nodes[2].clips[0].data.clone());
    s.register_asset("c.wav", s.graph().nodes[3].clips[0].data.clone());
    let (json, _) = s.to_json(SR).unwrap();
    struct Loader(Vec<(String, Arc<AudioData>)>);
    impl daw_session::project::AudioLoader for Loader {
        fn load(&self, path: &str) -> Result<Arc<AudioData>, String> {
            self.0.iter().find(|(p, _)| p == path).map(|(_, d)| d.clone()).ok_or_else(|| "absent".into())
        }
    }
    let loader = Loader(s.assets().iter().map(|(d, p)| (p.clone(), d.clone())).collect());
    let (back, rate) = Session::from_json(&json, &loader).unwrap();
    assert_eq!(rate, SR);
    assert_eq!(hash(&back), hash(&s));
    assert_eq!(back.graph().nodes[a.0 as usize].output, Some(y));
}

#[test]
fn track_inputs_follow_their_track_through_renumbering_and_undo() {
    use daw_session::nodes::NodeInput;
    let (mut s, [_, a, b, c, _]) = project();
    assert_eq!(s.node_input(a), NodeInput::default(), "audio mono, canal 1, écoute directe");
    let vocal = NodeInput { midi: false, channels: [3, 4], stereo: true, monitor: false };
    let keys = NodeInput { midi: true, channels: [0, 1], stereo: false, monitor: true };
    assert!(s.set_node_input_untracked(b, vocal));
    assert!(s.set_node_input_untracked(c, keys));
    assert!(!s.set_node_input_untracked(NodeId(99), keys));
    s.remove_node(a, &mut keep).unwrap();
    assert_eq!((s.node_input(NodeId(1)), s.node_input(NodeId(2))), (vocal, keys), "B et C ont été renumérotés, leurs entrées les suivent");
    s.undo();
    assert_eq!((s.node_input(a), s.node_input(b), s.node_input(c)), (NodeInput::default(), vocal, keys));
    // Une piste supprimée revient avec son entrée.
    s.remove_node(b, &mut keep).unwrap();
    s.undo();
    assert_eq!(s.node_input(b), vocal);
    assert!(s.undo_label().is_none() || s.undo_label() != Some("Entrée"), "l'entrée n'est pas dans l'historique");
}
