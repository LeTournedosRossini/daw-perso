//! Sélection multiple : déplacer, supprimer, copier / couper / coller et dupliquer plusieurs clips (audio et notes mêlés),
//! chaque opération en une seule action annulable.

use daw_engine::Engine;
use daw_ui::demo::demo_session;
use daw_ui::protocol::{ClipInfo, NodeInfo, ProjectInfo};
use daw_ui::{Event, Host};

const SR: u32 = 48_000;

fn host() -> Host {
    let (_, handle) = Engine::new(SR, 256);
    let (session, meta) = demo_session(SR);
    Host::new(handle, session, meta).unwrap()
}

fn project(ev: &[Event]) -> &ProjectInfo {
    ev.iter().rev().find_map(|e| if let Event::Project(p) = e { Some(p) } else { None }).expect("événement project")
}

fn current(h: &mut Host) -> ProjectInfo {
    project(&h.command_json(r#"{"type":"get_project"}"#)).clone()
}

fn error(ev: &[Event]) -> Option<String> {
    ev.iter().find_map(|e| if let Event::Error { message, .. } = e { Some(message.clone()) } else { None })
}

fn undo_label(ev: &[Event]) -> Option<String> {
    ev.iter().rev().find_map(|e| if let Event::History { undo_label, .. } = e { Some(undo_label.clone()) } else { None }).flatten()
}

fn selection(ev: &[Event]) -> Vec<u64> {
    ev.iter().find_map(|e| if let Event::Selection { clips } = e { Some(clips.clone()) } else { None }).expect("événement selection")
}

/// (id, début, longueur, nom) de tous les clips de tous les nœuds, avec le numéro du nœud.
fn all_clips(p: &ProjectInfo) -> Vec<(u64, u32, u64, u64, String)> {
    let mut out = Vec::new();
    for n in &p.nodes {
        for c in &n.clips {
            match c {
                ClipInfo::Audio { id, start, len, name, .. } | ClipInfo::Midi { id, start, len, name, .. } => out.push((*id, n.id, *start, *len, name.clone())),
            }
        }
    }
    out
}

fn node<'a>(p: &'a ProjectInfo, id: u32) -> &'a NodeInfo {
    p.nodes.iter().find(|n| n.id == id).unwrap()
}

fn ids_json(ids: &[u64]) -> String {
    format!("[{}]", ids.iter().map(u64::to_string).collect::<Vec<_>>().join(","))
}

/// Les deux premiers clips audio de la démonstration, et le clip de notes.
fn picks(p: &ProjectInfo) -> (u64, u64, u64) {
    let clips = all_clips(p);
    let audio: Vec<u64> = clips.iter().filter(|c| c.0 < 1_000_000).map(|c| c.0).collect();
    let midi = clips.iter().find(|c| c.0 >= 1_000_000).map(|c| c.0).expect("un clip de notes");
    (audio[0], audio[1], midi)
}

fn start_of(p: &ProjectInfo, id: u64) -> u64 {
    all_clips(p).iter().find(|c| c.0 == id).unwrap().2
}

#[test]
fn moving_a_mixed_group_keeps_their_spacing_stops_at_zero_and_is_one_undo_step() {
    let mut h = host();
    let p = current(&mut h);
    let (a, b, m) = picks(&p);
    let (sa, sb, sm) = (start_of(&p, a), start_of(&p, b), start_of(&p, m));
    let ev = h.command_json(&format!(r#"{{"type":"move_clips","clips":{},"delta":4800}}"#, ids_json(&[a, b, m])));
    assert!(error(&ev).is_none(), "{ev:?}");
    let q = project(&ev);
    assert_eq!((start_of(q, a), start_of(q, b)), (sa + 4800, sb + 4800));
    assert_eq!(start_of(q, m), sm + 4800, "le clip de notes suit, au tick près");
    assert_eq!(undo_label(&ev).as_deref(), Some("Déplacer des clips"));
    // Un seul pas d'annulation.
    let back = h.command_json(r#"{"type":"undo"}"#);
    let r = project(&back);
    assert_eq!((start_of(r, a), start_of(r, b), start_of(r, m)), (sa, sb, sm));
    // Vers la gauche au-delà de 0 : le groupe s'arrête à 0 sans changer ses écarts.
    let ev = h.command_json(&format!(r#"{{"type":"move_clips","clips":{},"delta":-100000000}}"#, ids_json(&[a, b])));
    assert!(error(&ev).is_none(), "{ev:?}");
    let q = project(&ev);
    let (na, nb) = (start_of(q, a), start_of(q, b));
    assert_eq!(na.min(nb), 0);
    assert_eq!(nb as i64 - na as i64, sb as i64 - sa as i64);
}

#[test]
fn deleting_several_clips_is_one_step_and_an_unknown_clip_refuses_everything() {
    let mut h = host();
    let p = current(&mut h);
    let (a, b, m) = picks(&p);
    let before = all_clips(&p).len();
    let ev = h.command_json(&format!(r#"{{"type":"delete_clips","clips":{}}}"#, ids_json(&[a, b, 999_999_999])));
    assert!(error(&ev).unwrap().contains("clip inconnu"));
    assert_eq!(all_clips(&current(&mut h)).len(), before, "rien n'a changé");
    let ev = h.command_json(&format!(r#"{{"type":"delete_clips","clips":{}}}"#, ids_json(&[a, b, m])));
    assert_eq!(all_clips(project(&ev)).len(), before - 3);
    assert_eq!(undo_label(&ev).as_deref(), Some("Supprimer des clips"));
    let back = h.command_json(r#"{"type":"undo"}"#);
    assert_eq!(all_clips(project(&back)).len(), before);
}

#[test]
fn copy_then_paste_places_the_earliest_clip_at_the_position_and_keeps_tracks_and_names() {
    let mut h = host();
    let p = current(&mut h);
    let (a, b, m) = picks(&p);
    let ca = all_clips(&p).into_iter().find(|c| c.0 == a).unwrap();
    let cb = all_clips(&p).into_iter().find(|c| c.0 == b).unwrap();
    let ev = h.command_json(&format!(r#"{{"type":"copy_clips","clips":{}}}"#, ids_json(&[a, b, m])));
    assert!(ev.iter().any(|e| matches!(e, Event::Clipboard { count: 3 })), "{ev:?}");
    // Copier ne change rien et n'entre pas dans l'historique.
    assert_eq!(all_clips(&current(&mut h)).len(), all_clips(&p).len());
    let earliest = [ca.2, cb.2, all_clips(&p).iter().find(|c| c.0 == m).unwrap().2].into_iter().min().unwrap();
    let at = 1_000_000;
    let ev = h.command_json(&format!(r#"{{"type":"paste_clips","at":{at}}}"#));
    assert!(error(&ev).is_none(), "{ev:?}");
    let sel = selection(&ev);
    assert_eq!(sel.len(), 3);
    let q = project(&ev);
    assert_eq!(undo_label(&ev).as_deref(), Some("Coller des clips"));
    let new_a = all_clips(q).into_iter().find(|c| c.0 == sel[0]).unwrap();
    assert_eq!(new_a.1, ca.1, "même piste");
    assert_eq!(new_a.2, at + (ca.2 - earliest), "écart au plus ancien conservé");
    assert_eq!(new_a.3, ca.3, "même longueur");
    assert_eq!(new_a.4, ca.4, "même nom");
    // Les originaux n'ont pas bougé et il y a trois clips de plus.
    assert_eq!(all_clips(q).len(), all_clips(&p).len() + 3);
    assert_eq!(start_of(q, a), ca.2);
    // Annuler retire les trois d'un coup ; le presse-papiers sert encore.
    let back = h.command_json(r#"{"type":"undo"}"#);
    assert_eq!(all_clips(project(&back)).len(), all_clips(&p).len());
    let again = h.command_json(&format!(r#"{{"type":"paste_clips","at":{}}}"#, at * 2));
    assert_eq!(selection(&again).len(), 3);
}

#[test]
fn cut_is_copy_then_delete_and_the_clipboard_survives_the_deletion() {
    let mut h = host();
    let p = current(&mut h);
    let (a, _, _) = picks(&p);
    let ca = all_clips(&p).into_iter().find(|c| c.0 == a).unwrap();
    h.command_json(&format!(r#"{{"type":"copy_clips","clips":[{a}]}}"#));
    h.command_json(&format!(r#"{{"type":"delete_clips","clips":[{a}]}}"#));
    let ev = h.command_json(r#"{"type":"paste_clips","at":2000000}"#);
    let sel = selection(&ev);
    let pasted = all_clips(project(&ev)).into_iter().find(|c| c.0 == sel[0]).unwrap();
    assert_eq!((pasted.1, pasted.2, pasted.3), (ca.1, 2_000_000, ca.3));
}

#[test]
fn paste_on_another_track_and_the_refusals() {
    let mut h = host();
    let p = current(&mut h);
    let (a, _b, _) = picks(&p);
    let ca = all_clips(&p).into_iter().find(|c| c.0 == a).unwrap();
    // Presse-papiers vide.
    assert!(error(&h.command_json(r#"{"type":"paste_clips","at":0}"#)).unwrap().contains("vide"));
    h.command_json(&format!(r#"{{"type":"copy_clips","clips":[{a}]}}"#));
    let other = p.nodes.iter().find(|n| n.id != ca.1 && n.kind != "bus" && n.kind != "master" && !n.clips.is_empty()).map(|n| n.id).expect("une autre piste");
    let ev = h.command_json(&format!(r#"{{"type":"paste_clips","at":5000,"node":{other}}}"#));
    let sel = selection(&ev);
    assert!(node(project(&ev), other).clips.iter().any(|c| matches!(c, ClipInfo::Audio { id, .. } if *id == sel[0])));
    // Vers la sortie principale : refusé, sans trace.
    let bus = p.nodes.iter().find(|n| n.kind == "master").map(|n| n.id).expect("la sortie principale");
    let n_before = all_clips(&current(&mut h)).len();
    assert!(error(&h.command_json(&format!(r#"{{"type":"paste_clips","at":5000,"node":{bus}}}"#))).is_some());
    assert_eq!(all_clips(&current(&mut h)).len(), n_before);
    // Nouveau projet : le presse-papiers est vidé.
    h.command_json(r#"{"type":"new_project"}"#);
    assert!(error(&h.command_json(r#"{"type":"paste_clips","at":0}"#)).unwrap().contains("vide"));
}

#[test]
fn duplicate_goes_right_after_the_group_or_by_the_given_delta() {
    let mut h = host();
    let p = current(&mut h);
    let (a, b, m) = picks(&p);
    let clips = all_clips(&p);
    let group: Vec<_> = [a, b, m].iter().map(|id| clips.iter().find(|c| c.0 == *id).unwrap().clone()).collect();
    let start = group.iter().map(|c| c.2).min().unwrap();
    let end = group.iter().map(|c| c.2 + c.3).max().unwrap();
    let ev = h.command_json(&format!(r#"{{"type":"duplicate_clips","clips":{}}}"#, ids_json(&[a, b, m])));
    assert!(error(&ev).is_none(), "{ev:?}");
    let sel = selection(&ev);
    assert_eq!(sel.len(), 3);
    assert_eq!(undo_label(&ev).as_deref(), Some("Dupliquer des clips"));
    let q = project(&ev);
    let copy_start = sel.iter().map(|id| start_of(q, *id)).min().unwrap();
    assert!((copy_start as i64 - end as i64).abs() <= 1, "la copie commence là où finit le groupe ({copy_start} / {end})");
    assert_eq!(all_clips(q).len(), clips.len() + 3);
    // Avec un décalage explicite (Alt + glisser).
    let ev = h.command_json(&format!(r#"{{"type":"duplicate_clips","clips":[{a}],"delta":{}}}"#, 12_345));
    let s1 = selection(&ev);
    assert_eq!(start_of(project(&ev), s1[0]), group[0].2 + 12_345);
    let _ = start;
}

#[test]
fn the_group_commands_validate_their_arguments() {
    let mut h = host();
    for bad in [
        r#"{"type":"move_clips","clips":[],"delta":5}"#,
        r#"{"type":"delete_clips","clips":[]}"#,
        r#"{"type":"duplicate_clips","clips":[]}"#,
        r#"{"type":"split_clips","clips":[],"at":5}"#,
        r#"{"type":"split_clips","clips":[1],"at":-5}"#,
        r#"{"type":"move_clips","clips":[1],"delta":0,"node_delta":1.5}"#,
        r#"{"type":"paste_clips","at":-1}"#,
        r#"{"type":"move_clips","clips":[1],"delta":"x"}"#,
    ] {
        assert!(error(&h.command_json(bad)).is_some(), "{bad}");
    }
}

fn track_ids(p: &ProjectInfo) -> Vec<u32> {
    p.nodes.iter().filter(|n| n.kind == "mono" || n.kind == "stereo").map(|n| n.id).collect()
}

fn owner(p: &ProjectInfo, clip: u64) -> u32 {
    all_clips(p).iter().find(|c| c.0 == clip).unwrap().1
}

#[test]
fn a_group_moves_down_and_up_by_whole_tracks_keeping_its_layout_and_is_one_step() {
    let mut h = host();
    let p = current(&mut h);
    let tracks = track_ids(&p);
    let (a, b, m) = picks(&p);
    let (oa, ob, om) = (owner(&p, a), owner(&p, b), owner(&p, m));
    let ia = |t: u32| tracks.iter().position(|x| *x == t).unwrap();
    let ev = h.command_json(&format!(r#"{{"type":"move_clips","clips":{},"delta":0,"node_delta":1}}"#, ids_json(&[a, b, m])));
    assert!(error(&ev).is_none(), "{ev:?}");
    let q = project(&ev);
    for (c, o) in [(a, oa), (b, ob), (m, om)] {
        assert_eq!(ia(owner(q, c)), (ia(o) + 1).min(tracks.len() - 1).max(ia(o)), "chaque clip descend d'une piste (les écarts de pistes sont conservés)");
    }
    assert_eq!(undo_label(&ev).as_deref(), Some("Déplacer des clips"));
    // Le clip qui n'est pas sur la dernière piste est vraiment descendu d'une piste (le test ne se contente pas d'un décalage nul).
    let movable = [(a, oa), (b, ob), (m, om)].into_iter().filter(|(_, o)| ia(*o) + 1 < tracks.len()).count();
    let moved = [(a, oa), (b, ob), (m, om)].into_iter().filter(|(c, o)| ia(owner(q, *c)) == ia(*o) + 1).count();
    assert!(moved > 0 || movable == 0, "au moins un clip est descendu ({moved}/{movable})");
    let back = h.command_json(r#"{"type":"undo"}"#);
    assert_eq!((owner(project(&back), a), owner(project(&back), b), owner(project(&back), m)), (oa, ob, om));
    // Trop bas : borné pour que tous restent sur une piste, sans erreur et sans changer leurs écarts.
    let ev = h.command_json(&format!(r#"{{"type":"move_clips","clips":{},"delta":0,"node_delta":100}}"#, ids_json(&[a, b, m])));
    assert!(error(&ev).is_none(), "{ev:?}");
    let q = project(&ev);
    let idx: Vec<usize> = [a, b, m].iter().map(|c| ia(owner(q, *c))).collect();
    assert_eq!(*idx.iter().max().unwrap(), tracks.len() - 1, "le plus bas du groupe est sur la dernière piste");
    let before: Vec<i64> = [oa, ob, om].iter().map(|o| ia(*o) as i64).collect();
    let spread = |v: &[i64]| v.iter().max().unwrap() - v.iter().min().unwrap();
    assert_eq!(spread(&idx.iter().map(|i| *i as i64).collect::<Vec<_>>()), spread(&before));
    // Horizontal et vertical en même temps.
    let ev = h.command_json(&format!(r#"{{"type":"move_clips","clips":{},"delta":4800,"node_delta":-100}}"#, ids_json(&[a, b])));
    assert!(error(&ev).is_none(), "{ev:?}");
}

#[test]
fn split_clips_cuts_every_selected_clip_under_the_position_in_one_step() {
    let mut h = host();
    let p = current(&mut h);
    let clips = all_clips(&p);
    // Une position qui tombe à l'intérieur de plusieurs clips (audio et notes) à la fois.
    let at = clips.iter().map(|c| c.2).max().unwrap() + 100;
    let under: Vec<u64> = clips.iter().filter(|c| c.2 < at && at < c.2 + c.3).map(|c| c.0).collect();
    assert!(under.len() >= 2, "la démonstration a au moins deux clips qui se recouvrent");
    let outside: Vec<u64> = clips.iter().filter(|c| !(c.2 < at && at < c.2 + c.3)).map(|c| c.0).take(1).collect();
    let all: Vec<u64> = under.iter().chain(outside.iter()).copied().collect();
    let ev = h.command_json(&format!(r#"{{"type":"split_clips","clips":{},"at":{at}}}"#, ids_json(&all)));
    assert!(error(&ev).is_none(), "{ev:?}");
    assert_eq!(undo_label(&ev).as_deref(), Some("Découper"));
    let q = project(&ev);
    assert_eq!(all_clips(q).len(), clips.len() + under.len(), "seuls les clips sous la position sont coupés");
    let sel = selection(&ev);
    assert_eq!(sel.len(), under.len() * 2, "les deux moitiés de chaque clip");
    for id in &under {
        let left = all_clips(q).into_iter().find(|c| c.0 == *id).unwrap();
        assert_eq!(left.2 + left.3, at, "la moitié gauche finit à la coupure (au tick près pour les notes)");
    }
    // Les moitiés droites gardent le nom.
    for id in &under {
        let name = &clips.iter().find(|c| c.0 == *id).unwrap().4;
        let right = sel.iter().filter(|s| !under.contains(s)).filter_map(|s| all_clips(q).into_iter().find(|c| c.0 == *s)).any(|c| &c.4 == name);
        assert!(right, "une moitié droite porte le nom {name}");
    }
    let back = h.command_json(r#"{"type":"undo"}"#);
    assert_eq!(all_clips(project(&back)).len(), clips.len());
    // Aucun clip ne contient la position : erreur, rien ne change.
    let ev = h.command_json(&format!(r#"{{"type":"split_clips","clips":{},"at":{}}}"#, ids_json(&outside), at + 500_000_000));
    assert!(error(&ev).unwrap().contains("aucun clip"));
    assert_eq!(all_clips(&current(&mut h)).len(), clips.len());
}

#[test]
fn paste_on_a_chosen_track_keeps_the_layout_of_several_source_tracks_and_refuses_when_tracks_run_out() {
    let mut h = host();
    let p = current(&mut h);
    let tracks = track_ids(&p);
    let clips = all_clips(&p);
    // Deux clips de deux pistes différentes, la plus haute en premier dans l'ordre d'affichage.
    let pos = |c: &(u64, u32, u64, u64, String)| tracks.iter().position(|t| *t == c.1);
    let mut pair: Vec<_> = clips.iter().filter(|c| pos(c).is_some()).cloned().collect();
    pair.sort_by_key(|c| pos(c).unwrap());
    let (top, bottom) = (pair.first().unwrap().clone(), pair.iter().find(|c| pos(c) != pos(pair.first().unwrap())).unwrap().clone());
    let gap = pos(&bottom).unwrap() - pos(&top).unwrap();
    h.command_json(&format!(r#"{{"type":"copy_clips","clips":[{},{}]}}"#, bottom.0, top.0));
    // Sur la deuxième piste : la plus haute des deux vient dessus, l'autre garde son écart.
    let dest = tracks[1];
    if 1 + gap < tracks.len() {
        let ev = h.command_json(&format!(r#"{{"type":"paste_clips","at":3000000,"node":{dest}}}"#));
        assert!(error(&ev).is_none(), "{ev:?}");
        let sel = selection(&ev);
        let q = project(&ev);
        let (n0, n1) = (owner(q, sel[0]), owner(q, sel[1]));
        // sel[0] = copie du clip « bottom » (copié en premier), sel[1] = copie du clip « top ».
        assert_eq!(n1, dest);
        assert_eq!(tracks.iter().position(|t| *t == n0).unwrap(), 1 + gap);
        h.command_json(r#"{"type":"undo"}"#);
    }
    // Trop bas : refusé sans trace.
    let last = *tracks.last().unwrap();
    if gap > 0 {
        let n = all_clips(&current(&mut h)).len();
        let ev = h.command_json(&format!(r#"{{"type":"paste_clips","at":0,"node":{last}}}"#));
        assert!(error(&ev).unwrap().contains("pas assez de pistes"), "{ev:?}");
        assert_eq!(all_clips(&current(&mut h)).len(), n);
    }
}
