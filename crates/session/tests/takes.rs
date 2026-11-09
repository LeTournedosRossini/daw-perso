use std::sync::Arc;

use daw_core_types::NodeId;
use daw_engine::{AudioData, Clip, GraphDesc, TrackFormat};
use daw_offline_render::{render, RenderSpec};
use daw_session::Session;

const SR: u32 = 48_000;

fn constant(v: f32, n: usize) -> Arc<AudioData> {
    AudioData::new(SR, vec![vec![v; n], vec![v; n]]).unwrap()
}

fn setup() -> (Session, NodeId) {
    let mut g = GraphDesc::new();
    let t = g.add_track("t", TrackFormat::Stereo);
    let mut s = Session::new(g);
    s.set_seam_ms(0); // niveaux exacts de part et d'autre des coupures ; les raccords ont leurs propres tests
    (s, t)
}

fn out(s: &Session) -> Vec<f32> {
    render(&RenderSpec::new(SR, 3_000, s.graph().clone())).unwrap().left
}

fn hash(s: &Session) -> u64 {
    render(&RenderSpec::new(SR, 3_000, s.graph().clone())).unwrap().hash()
}

#[test]
fn recording_over_part_of_a_clip_splits_it_and_keeps_the_part_underneath_as_a_muted_take() {
    let (mut s, t) = setup();
    s.add_clip(t, Clip::whole(constant(0.25, 2_000), 0)).unwrap();
    let before = hash(&s);
    let take = s.record_over(t, Clip::new(constant(0.5, 500), 800, 0, 500, 1.0)).unwrap();
    let o = out(&s);
    assert!((o[100] - 0.25).abs() < 1e-6, "avant la zone : l'ancien clip");
    assert!((o[1_000] - 0.5).abs() < 1e-6, "dans la zone : seulement la nouvelle prise (l'ancienne est muette)");
    assert!((o[1_500] - 0.25).abs() < 1e-6, "après la zone : l'ancien clip");
    let members = s.take_group_members(take);
    assert_eq!(members.len(), 2);
    assert_eq!(s.graph().nodes[t.0 as usize].clips.len(), 4, "gauche, dessous (muet), droite, nouvelle prise");
    // Un seul pas d'annulation.
    assert_eq!(s.undo_label(), Some("Enregistrer une prise"));
    assert!(s.undo());
    assert_eq!(hash(&s), before);
    assert_eq!(s.graph().nodes[t.0 as usize].clips.len(), 1);
    assert!(s.redo());
    assert!((out(&s)[1_000] - 0.5).abs() < 1e-6);
}

#[test]
fn switching_the_active_take_is_one_undo_step_and_swaps_the_sound() {
    let (mut s, t) = setup();
    s.add_clip(t, Clip::whole(constant(0.25, 2_000), 0)).unwrap();
    let take = s.record_over(t, Clip::new(constant(0.5, 500), 800, 0, 500, 1.0)).unwrap();
    let old = s.take_group_members(take).into_iter().find(|&c| c != take).unwrap();
    let with_new = hash(&s);
    s.set_active_take(old).unwrap();
    let o = out(&s);
    assert!((o[1_000] - 0.25).abs() < 1e-6, "l'ancien passage est actif, la nouvelle prise muette");
    assert_eq!(s.undo_label(), Some("Changer de prise"));
    assert!(s.undo());
    assert_eq!(hash(&s), with_new);
    assert!(s.redo());
    assert!((out(&s)[1_000] - 0.25).abs() < 1e-6);
    // Déjà active : aucune action.
    let steps = s.undo_label().map(str::to_owned);
    s.set_active_take(old).unwrap();
    assert_eq!(s.undo_label().map(str::to_owned), steps);
}

#[test]
fn a_second_take_over_the_same_zone_joins_the_same_group_and_a_clip_alone_cannot_be_activated() {
    let (mut s, t) = setup();
    let alone = s.add_clip(t, Clip::whole(constant(0.25, 2_000), 0)).unwrap();
    assert!(s.set_active_take(alone).is_err(), "hors groupe");
    s.record_over(t, Clip::new(constant(0.5, 500), 800, 0, 500, 1.0)).unwrap();
    let third = s.record_over(t, Clip::new(constant(0.75, 500), 800, 0, 500, 1.0)).unwrap();
    assert!((out(&s)[1_000] - 0.75).abs() < 1e-6);
    assert_eq!(s.take_group_members(third).len(), 3);
}

#[test]
fn nothing_underneath_means_no_group_and_a_take_that_spans_two_clips_cuts_both() {
    let (mut s, t) = setup();
    let first = s.record_over(t, Clip::whole(constant(0.25, 1_000), 0)).unwrap();
    assert_eq!(s.take_group_members(first), vec![first]);
    s.add_clip(t, Clip::whole(constant(0.125, 1_000), 1_000)).unwrap();
    let take = s.record_over(t, Clip::new(constant(0.5, 600), 800, 0, 600, 1.0)).unwrap();
    let o = out(&s);
    assert!((o[900] - 0.5).abs() < 1e-6 && (o[1_200] - 0.5).abs() < 1e-6);
    assert!((o[700] - 0.25).abs() < 1e-6 && (o[1_500] - 0.125).abs() < 1e-6);
    assert_eq!(s.take_group_members(take).len(), 3);
}

struct Fixed(Arc<AudioData>);
impl daw_session::project::AudioLoader for Fixed {
    fn load(&self, _: &str) -> Result<Arc<AudioData>, String> {
        Ok(self.0.clone())
    }
}

#[test]
fn muted_and_group_survive_a_project_round_trip() {
    let (mut s, t) = setup();
    let old = s.register_asset("audio/a.wav", constant(0.25, 2_000));
    let new = s.register_asset("audio/b.wav", constant(0.5, 500));
    s.add_clip(t, Clip::whole(old, 0)).unwrap();
    s.record_over(t, Clip::new(new, 800, 0, 500, 1.0)).unwrap();
    let (json, _) = s.to_json(SR).unwrap();
    assert!(json.contains("\"muted\": true") && json.contains("\"take_group\": 1"));
    // Les deux fichiers sont ici le même signal : seul compte ce qui est muet / groupé.
    let (back, _) = Session::from_json(&json, &Fixed(constant(0.25, 2_000))).unwrap();
    let flags = |s: &Session| s.graph().nodes[1].clips.iter().map(|c| (c.start, c.len, c.muted, c.take_group)).collect::<Vec<_>>();
    assert_eq!(flags(&back), flags(&s));
}

#[test]
fn composing_activates_only_a_range_of_an_inactive_take_and_leaves_the_rest_of_the_active_one() {
    let (mut s, t) = setup();
    s.add_clip(t, Clip::whole(constant(0.25, 2_000), 0)).unwrap();
    let new = s.record_over(t, Clip::new(constant(0.5, 500), 800, 0, 500, 1.0)).unwrap();
    let old = s.take_group_members(new).into_iter().find(|&c| c != new).unwrap();
    let before = hash(&s);
    s.activate_take_range(old, 900, 1_000).unwrap();
    let o = out(&s);
    assert!((o[100] - 0.25).abs() < 1e-6, "avant : l'ancien clip");
    assert!((o[850] - 0.5).abs() < 1e-6, "début de la nouvelle prise, resté actif");
    assert!((o[950] - 0.25).abs() < 1e-6, "la plage composée : l'ancien passage");
    assert!((o[1_100] - 0.5).abs() < 1e-6, "fin de la nouvelle prise, restée active");
    assert!((o[1_500] - 0.25).abs() < 1e-6, "après : l'ancien clip");
    assert_eq!(s.undo_label(), Some("Composer une prise"));
    assert!(s.undo());
    assert_eq!(hash(&s), before, "une seule action annulable");
    // Une plage hors de la prise ou vide est refusée sans rien changer.
    assert!(s.activate_take_range(old, 10, 20).is_err());
    assert!(s.activate_take_range(old, 900, 900).is_err());
    assert_eq!(hash(&s), before);
}

#[test]
fn activating_a_take_that_only_partly_overlaps_the_active_one_cuts_the_active_one() {
    let (mut s, t) = setup();
    s.add_clip(t, Clip::whole(constant(0.25, 2_000), 0)).unwrap();
    let first = s.record_over(t, Clip::new(constant(0.5, 500), 800, 0, 500, 1.0)).unwrap();
    // Seconde prise décalée : elle recouvre la fin de la première et un peu de l'ancien clip.
    let second = s.record_over(t, Clip::new(constant(0.75, 500), 1_100, 0, 500, 1.0)).unwrap();
    assert!((out(&s)[1_200] - 0.75).abs() < 1e-6);
    // Rendre la première active : le morceau de la première qui est resté sous la seconde reprend sa place [800, 1100) ;
    // la seconde est coupée là où elles se recouvrent et reste active au-delà.
    s.set_active_take(first).unwrap();
    let o = out(&s);
    assert!((o[1_000] - 0.5).abs() < 1e-6, "la première reprend [800, 1100)");
    assert!((o[1_250] - 0.75).abs() < 1e-6 && (o[1_400] - 0.75).abs() < 1e-6, "la seconde reste active à partir de 1 100");
    assert!(s.clip(second).is_some());
}

#[test]
fn loop_passes_each_cover_the_previous_so_the_last_is_active_and_a_partial_last_one_keeps_the_rest_of_the_previous() {
    let (mut s, t) = setup();
    let first = Clip::new(constant(0.25, 1_000), 500, 0, 1_000, 1.0);
    let second = Clip::new(constant(0.5, 1_000), 500, 0, 1_000, 1.0);
    let partial = Clip::new(constant(0.75, 300), 500, 0, 300, 1.0);
    let ids = s.record_passes(t, vec![first, second, partial]).unwrap();
    assert_eq!(ids.len(), 3);
    let o = out(&s);
    assert!((o[600] - 0.75).abs() < 1e-6, "le dernier passage (interrompu) joue sur ce qu'il couvre");
    assert!((o[1_000] - 0.5).abs() < 1e-6, "le reste du passage précédent reste actif");
    assert_eq!(s.undo_label(), Some("Enregistrer"));
    assert!(s.undo());
    assert!(s.graph().nodes[t.0 as usize].clips.is_empty(), "une seule action annulable");
    // Un seul passage : une prise ordinaire, sans groupe.
    let (mut s2, t2) = setup();
    let ids = s2.record_passes(t2, vec![Clip::new(constant(0.25, 1_000), 0, 0, 1_000, 1.0)]).unwrap();
    assert_eq!(s2.take_group_members(ids[0]), vec![ids[0]]);
    assert!(s2.record_passes(t2, vec![]).unwrap().is_empty());
}

#[test]
fn seams_between_take_pieces_are_short_crossfades_with_no_step_and_one_undo_step() {
    let mut g = GraphDesc::new();
    let t = g.add_track("t", TrackFormat::Stereo);
    let mut s = Session::new(g);
    s.set_seam_ms(10); // 480 échantillons à 48 kHz
    s.add_clip(t, Clip::whole(constant(0.25, 4_000), 0)).unwrap();
    let before = hash(&s);
    // Une prise d'un autre niveau posée au milieu, qui recouvre l'ancien clip sur [1 000, 2 500) : deux jonctions.
    let take = Clip::new(constant(0.5, 1_500), 1_000, 0, 1_500, 1.0);
    s.record_over(t, take).unwrap();
    let o = render(&RenderSpec::new(SR, 4_000, s.graph().clone())).unwrap().left;
    assert!((o[500] - 0.25).abs() < 1e-6 && (o[1_800] - 0.5).abs() < 1e-3 && (o[3_500] - 0.25).abs() < 1e-6, "niveaux hors raccords");
    // Pas de marche brutale : sans raccord on sauterait de 0,25 à 0,5 en un échantillon aux deux jonctions.
    let max_step = o.windows(2).map(|w| (w[1] - w[0]).abs()).fold(0.0f32, f32::max);
    assert!(max_step < 0.01, "plus grande marche : {max_step}");
    assert_eq!(s.undo_label(), Some("Enregistrer une prise"));
    assert!(s.undo());
    assert_eq!(hash(&s), before, "raccords compris, une seule action annulable");
    // Raccords coupés : coupure franche (un saut de 0,25).
    let (mut s2, t2) = setup();
    s2.add_clip(t2, Clip::whole(constant(0.25, 4_000), 0)).unwrap();
    s2.record_over(t2, Clip::new(constant(0.5, 1_500), 1_000, 0, 1_500, 1.0)).unwrap();
    let o2 = out(&s2);
    assert!(o2.windows(2).map(|w| (w[1] - w[0]).abs()).fold(0.0f32, f32::max) > 0.2);
}

#[test]
fn a_seam_never_makes_a_fade_longer_than_a_very_short_neighbour_and_the_graph_stays_valid() {
    // Régression : le premier passage d'une boucle dépasse de la latence compensée avant la zone ; activer le deuxième passage laisse
    // un morceau de 100 échantillons contre un clip long, et le fondu de raccord (480) dépassait ce morceau : le moteur refusait le graphe.
    let mut g = GraphDesc::new();
    let t = g.add_track("t", TrackFormat::Stereo);
    let mut s = Session::new(g);
    s.set_seam_ms(10);
    let data = constant(0.25, 6_000);
    let first = Clip::new(data.clone(), 900, 0, 1_100, 1.0);
    let second = Clip::new(data.clone(), 1_000, 1_100, 1_000, 1.0);
    let third = Clip::new(data, 1_000, 2_100, 600, 1.0);
    let ids = s.record_passes(t, vec![first, second, third]).unwrap();
    // (Le premier passage dépasse avant les autres : son petit bout de 100 échantillons reste hors groupe, c'est le cas visé.)
    for id in &ids[1..] {
        s.set_active_take(*id).unwrap();
        s.graph().validate().unwrap_or_else(|e| panic!("graphe refusé après activation de {id:?} : {e:?}"));
        render(&RenderSpec::new(SR, 3_000, s.graph().clone())).unwrap();
        s.set_active_take(*ids.last().unwrap()).unwrap();
        s.graph().validate().unwrap_or_else(|e| panic!("graphe refusé au retour du dernier : {e:?}"));
    }
    // Toute plage composée dans n'importe quelle prise reste valide.
    for (a, b) in [(1_000, 1_030), (1_050, 2_000), (950, 1_010), (1_990, 2_090)] {
        s.activate_take_range(ids[1], a, b).ok();
        s.graph().validate().unwrap_or_else(|e| panic!("plage {a}-{b} : {e:?}"));
    }
}
