//! Plugins sur une piste : inventaire, ajout, retrait, ordre, contournement, paramètres, latence dynamique, enregistrement.

use std::sync::Arc;

use daw_clap_host::{clap_insert_factory, Bundle};
use daw_engine::Engine;
use daw_test_plugin::{entry, PARAM_EXTRA_LATENCY, PARAM_GAIN, PLUGIN_ID};
use daw_ui::demo::demo_session;
use daw_ui::host::{PluginRef, PluginResolver};
use daw_ui::protocol::{InsertInfo, NodeInfo, PluginListing, ProjectInfo};
use daw_ui::{Event, Host};

const SR: u32 = 48_000;

fn pref() -> PluginRef {
    PluginRef { clap_id: PLUGIN_ID.to_str().unwrap().into(), vendor: "daw-perso".into(), name: "Gain de test".into(), file: "test.clap".into() }
}

fn resolver() -> PluginResolver {
    let bundle = Arc::new(Bundle::from_static_entry(entry()).unwrap());
    Box::new(move |e| (e.clap_id == PLUGIN_ID.to_str().unwrap()).then(|| (clap_insert_factory(Arc::clone(&bundle), e.clap_id.clone()), pref())))
}

fn host() -> (Engine, Host) {
    let (engine, handle) = Engine::new(SR, 64);
    let (session, meta) = demo_session(SR);
    let mut h = Host::new(handle, session, meta).unwrap();
    h.set_plugin_resolver(resolver());
    (engine, h)
}

fn run(engine: &mut Engine, blocks: usize) {
    let (mut l, mut r) = (vec![0.0f32; 64], vec![0.0f32; 64]);
    for _ in 0..blocks {
        engine.process(&mut l, &mut r);
    }
}

fn project(ev: &[Event]) -> &ProjectInfo {
    ev.iter().rev().find_map(|e| if let Event::Project(p) = e { Some(p) } else { None }).expect("événement project")
}

fn node<'a>(ev: &'a [Event], name: &str) -> &'a NodeInfo {
    project(ev).nodes.iter().find(|n| n.name == name).unwrap()
}

fn chain(ev: &[Event], track: &str) -> Vec<InsertInfo> {
    node(ev, track).inserts.clone()
}

fn error(ev: &[Event]) -> String {
    ev.iter().find_map(|e| if let Event::Error { message, .. } = e { Some(message.clone()) } else { None }).expect("une erreur")
}

fn undo_label(ev: &[Event]) -> Option<String> {
    ev.iter().rev().find_map(|e| if let Event::History { undo_label, .. } = e { Some(undo_label.clone()) } else { None }).flatten()
}

/// Enregistre le projet dans un dossier temporaire : il n'a plus de modifications non enregistrées.
fn saved(h: &mut Host, name: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("daw-plugins-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let ev = h.command_json(&format!(r#"{{"type":"save_project","path":{:?}}}"#, dir.join("P").to_string_lossy()));
    assert!(ev.iter().any(|e| matches!(e, Event::File { dirty: false, .. })));
    dir
}

fn add(h: &mut Host, node: u32) -> Vec<Event> {
    h.command_json(&format!(r#"{{"type":"add_plugin","node":{node},"clap_id":"com.daw-perso.test-gain","file":"test.clap"}}"#))
}

fn params(h: &mut Host, insert: u32) -> Vec<daw_ui::protocol::ParamEntry> {
    match h.command_json(&format!(r#"{{"type":"get_plugin_params","insert":{insert}}}"#)).remove(0) {
        Event::PluginParams { params, .. } => params,
        e => panic!("{e:?}"),
    }
}

#[test]
fn adding_a_plugin_puts_it_on_the_track_with_its_latency_and_is_undoable() {
    let (_, mut h) = host();
    let ev = add(&mut h, 2);
    let c = chain(&ev, "Basse");
    assert_eq!(c.len(), 1);
    assert_eq!((c[0].name.as_str(), c[0].vendor.as_str(), c[0].bypass, c[0].latency), ("Gain de test", "daw-perso", false, 64));
    assert_eq!(project(&ev).latency_samples, 64, "la compensation de latence en tient compte");
    assert_eq!(undo_label(&ev).as_deref(), Some("Ajouter un plugin"));
    let ev = add(&mut h, 2);
    assert_eq!(chain(&ev, "Basse").len(), 2);
    assert_ne!(chain(&ev, "Basse")[0].id, chain(&ev, "Basse")[1].id);
    h.command_json(r#"{"type":"undo"}"#);
    let ev = h.command_json(r#"{"type":"undo"}"#);
    assert!(chain(&ev, "Basse").is_empty());
    assert_eq!(project(&ev).latency_samples, 0);
    // Refus sans trace : plugin inconnu, piste inconnue.
    assert!(error(&h.command_json(r#"{"type":"add_plugin","node":2,"clap_id":"inconnu"}"#)).contains("introuvable"));
    assert!(error(&add(&mut h, 99)).contains("inconnu"));
    assert!(chain(&h.command_json(r#"{"type":"get_project"}"#), "Basse").is_empty());
}

#[test]
fn plugins_can_be_reordered_and_removed_and_removal_is_undone_with_the_state() {
    let (mut engine, mut h) = host();
    let a = chain(&add(&mut h, 2), "Basse")[0].id;
    let b = chain(&add(&mut h, 2), "Basse")[1].id;
    let c = chain(&add(&mut h, 2), "Basse")[2].id;
    let ids = |ev: &[Event]| chain(ev, "Basse").iter().map(|i| i.id).collect::<Vec<_>>();
    let ev = h.command_json(&format!(r#"{{"type":"move_plugin","insert":{c},"before":{a}}}"#));
    assert_eq!(ids(&ev), [c, a, b]);
    assert_eq!(undo_label(&ev).as_deref(), Some("Déplacer un plugin"));
    let ev = h.command_json(&format!(r#"{{"type":"move_plugin","insert":{c}}}"#));
    assert_eq!(ids(&ev), [a, b, c]);
    // Réglage du deuxième plugin, puis retrait et annulation : le réglage est revenu.
    h.command_json(&format!(r#"{{"type":"set_plugin_param","insert":{b},"param":{PARAM_GAIN},"value":0.25}}"#));
    run(&mut engine, 4);
    let ev = h.command_json(&format!(r#"{{"type":"remove_plugin","insert":{b}}}"#));
    assert_eq!(ids(&ev), [a, c]);
    assert_eq!(undo_label(&ev).as_deref(), Some("Retirer un plugin"));
    let ev = h.command_json(r#"{"type":"undo"}"#);
    assert_eq!(ids(&ev), [a, b, c], "même place, même identifiant");
    let gain = params(&mut h, b).into_iter().find(|p| p.id == PARAM_GAIN).unwrap();
    assert_eq!(gain.value, 0.25, "l'état du plugin est revenu avec lui");
    assert!(error(&h.command_json(r#"{"type":"remove_plugin","insert":999}"#)).contains("inconnu"));
    assert!(error(&h.command_json(&format!(r#"{{"type":"move_plugin","insert":{a},"before":999}}"#))).contains("inconnu"));
}

#[test]
fn bypass_is_shown_marks_the_project_unsaved_and_is_not_in_the_history() {
    let (_, mut h) = host();
    let id = chain(&add(&mut h, 2), "Basse")[0].id;
    let label_before = undo_label(&h.command_json(r#"{"type":"get_project"}"#));
    let dir = saved(&mut h, "bypass");
    let ev = h.command_json(&format!(r#"{{"type":"set_plugin_bypass","insert":{id},"bypass":true}}"#));
    assert!(chain(&ev, "Basse")[0].bypass);
    assert!(ev.iter().any(|e| matches!(e, Event::File { dirty: true, .. })));
    assert_eq!(undo_label(&h.command_json(r#"{"type":"get_project"}"#)), label_before);
    assert_eq!(project(&ev).latency_samples, 64, "contourné, il garde sa latence : rien ne se décale");
    assert!(error(&h.command_json(r#"{"type":"set_plugin_bypass","insert":999,"bypass":true}"#)).contains("inconnu"));
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn parameters_are_listed_with_their_values_and_can_be_set() {
    let (mut engine, mut h) = host();
    let id = chain(&add(&mut h, 2), "Basse")[0].id;
    let list = params(&mut h, id);
    let names: Vec<&str> = list.iter().map(|p| p.name.as_str()).collect();
    assert!(names.len() >= 2 && list.iter().any(|p| p.id == PARAM_GAIN) && list.iter().any(|p| p.id == PARAM_EXTRA_LATENCY), "{names:?}");
    let gain = list.iter().find(|p| p.id == PARAM_GAIN).unwrap();
    assert!(gain.min <= gain.default && gain.default <= gain.max);
    let dir = saved(&mut h, "param");
    let ev = h.command_json(&format!(r#"{{"type":"set_plugin_param","insert":{id},"param":{PARAM_GAIN},"value":0.5}}"#));
    assert!(matches!(ev[0], Event::PluginParam { insert, param, value, .. } if insert == id && param == PARAM_GAIN && value == 0.5));
    assert!(ev.iter().any(|e| matches!(e, Event::File { dirty: true, .. })));
    run(&mut engine, 4);
    assert_eq!(params(&mut h, id).into_iter().find(|p| p.id == PARAM_GAIN).unwrap().value, 0.5);
    assert!(error(&h.command_json(r#"{"type":"get_plugin_params","insert":999}"#)).contains("inconnu"));
    assert!(error(&h.command_json(r#"{"type":"set_plugin_param","insert":999,"param":0,"value":1}"#)).contains("inconnu"));
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn a_plugin_that_changes_its_latency_is_restarted_and_the_total_latency_follows() {
    let (mut engine, mut h) = host();
    let id = chain(&add(&mut h, 2), "Basse")[0].id;
    h.command_json(&format!(r#"{{"type":"set_plugin_param","insert":{id},"param":{PARAM_EXTRA_LATENCY},"value":100}}"#));
    run(&mut engine, 4); // le plugin reçoit le réglage dans le callback et demande à être relancé
    let ev = h.tick();
    let p = ev.iter().find_map(|e| if let Event::Project(p) = e { Some(p) } else { None }).expect("un événement project après la relance");
    assert_eq!(p.latency_samples, 164);
    assert_eq!(p.nodes.iter().find(|n| n.name == "Basse").unwrap().inserts[0].latency, 164);
}

#[test]
fn project_files_keep_the_chain_order_the_bypass_and_the_settings() {
    let dir = std::env::temp_dir().join(format!("daw-plugins-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let (mut engine, mut a) = host();
    let first = chain(&add(&mut a, 2), "Basse")[0].id;
    let second = chain(&add(&mut a, 2), "Basse")[1].id;
    a.command_json(&format!(r#"{{"type":"set_plugin_param","insert":{second},"param":{PARAM_GAIN},"value":0.125}}"#));
    a.command_json(&format!(r#"{{"type":"set_plugin_bypass","insert":{first},"bypass":true}}"#));
    run(&mut engine, 4);
    a.command_json(&format!(r#"{{"type":"save_project","path":{:?}}}"#, dir.join("P").to_string_lossy()));
    let (_, mut b) = host();
    let ev = b.command_json(&format!(r#"{{"type":"open_project","path":{:?}}}"#, dir.join("P.daw").to_string_lossy()));
    let c = chain(&ev, "Basse");
    assert_eq!(c.len(), 2);
    assert_eq!((c[0].bypass, c[1].bypass), (true, false));
    let reopened = params(&mut b, c[1].id).into_iter().find(|p| p.id == PARAM_GAIN).unwrap();
    assert_eq!(reopened.value, 0.125, "le réglage est revenu avec l'état du plugin");
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn the_plugin_catalog_is_scanned_in_the_background_once() {
    let (_, mut h) = host();
    h.set_catalog_scan(Box::new(|| {
        vec![
            PluginListing { clap_id: "a".into(), name: "AnalogSynth".into(), vendor: "Le Tournedos Rossini".into(), file: "AnalogSynth.clap".into(), description: String::new(), features: vec!["instrument".into()] },
            PluginListing { clap_id: "b".into(), name: "Chorus".into(), vendor: "Autre".into(), file: "Chorus.clap".into(), description: String::new(), features: vec![] },
        ]
    }));
    let first = h.command_json(r#"{"type":"list_plugins"}"#);
    assert!(matches!(&first[0], Event::Plugins { plugins, scanning: true } if plugins.is_empty()));
    // Le balayage rend sa liste par `tick`.
    let mut list = None;
    for _ in 0..200 {
        if let Some(Event::Plugins { plugins, scanning: false }) = h.tick().into_iter().find(|e| matches!(e, Event::Plugins { .. })) {
            list = Some(plugins);
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    let list = list.expect("le balayage s'est terminé");
    assert_eq!(list.iter().map(|p| p.name.as_str()).collect::<Vec<_>>(), ["AnalogSynth", "Chorus"]);
    // Ensuite la liste est en mémoire : réponse immédiate, sans nouveau balayage.
    assert!(matches!(&h.command_json(r#"{"type":"list_plugins"}"#)[0], Event::Plugins { plugins, scanning: false } if plugins.len() == 2));
}

/// Énergie de la sortie principale sur 10 blocs de lecture (Basse seule : les autres pistes sont muettes).
fn bass_energy(engine: &mut Engine, h: &mut Host) -> f32 {
    h.command_json(r#"{"type":"seek","pos":96000}"#);
    h.command_json(r#"{"type":"transport","action":"play"}"#);
    let (mut l, mut r) = (vec![0.0f32; 64], vec![0.0f32; 64]);
    for _ in 0..80 {
        engine.process(&mut l, &mut r);
    }
    let mut e = 0.0f32;
    for _ in 0..40 {
        engine.process(&mut l, &mut r);
        e += l.iter().chain(&r).map(|v| v * v).sum::<f32>();
    }
    h.command_json(r#"{"type":"transport","action":"stop"}"#);
    e
}

#[test]
fn an_automated_plugin_parameter_is_driven_by_its_curve_unless_the_lane_is_off_and_its_text_can_be_read() {
    let (mut engine, mut h) = host();
    // Seule la Basse sonne.
    let ev = h.command_json(r#"{"type":"get_project"}"#);
    for n in &project(&ev).nodes {
        if n.kind != "master" && n.name != "Basse" {
            h.command_json(&format!(r#"{{"type":"set_node","node":{},"mute":true}}"#, n.id));
        }
    }
    let basse = node(&ev, "Basse").id;
    let ev = h.command_json(&format!(r#"{{"type":"add_plugin","node":{basse},"clap_id":"{}","file":"test.clap"}}"#, PLUGIN_ID.to_str().unwrap()));
    let ins = chain(&ev, "Basse")[0].id;
    let baseline = bass_energy(&mut engine, &mut h);
    assert!(baseline > 0.0);
    // Courbe du gain du plugin à 0 : le plugin se tait (mode par défaut : trim, lu comme la lecture pour un plugin).
    let lane = format!(r#"{{"type":"set_lane","node":{basse},"target":{{"kind":"insert","insert":{ins},"param":{PARAM_GAIN}}},"points":[{{"pos":[0,1],"value":0,"curve":"linear"}}]}}"#);
    let ev = h.command_json(&lane);
    assert_eq!(node(&ev, "Basse").automation[0].mode, "trim");
    let muted = bass_energy(&mut engine, &mut h);
    assert!(muted < baseline * 0.001, "le paramètre automatisé commande le plugin : {baseline} → {muted}");
    // Voie désactivée : la courbe ne commande plus rien (le plugin garde la dernière valeur reçue : ici 0), et un réglage manuel
    // est maintenant obéi, ce qui n'était pas le cas tant que la courbe écrasait le paramètre.
    let ev = h.command_json(&format!(r#"{{"type":"set_lane_mode","node":{basse},"target":{{"kind":"insert","insert":{ins},"param":{PARAM_GAIN}}},"mode":"off"}}"#));
    assert_eq!(node(&ev, "Basse").automation[0].mode, "off", "{ev:?}");
    let ev = h.command_json(&format!(r#"{{"type":"set_plugin_param","insert":{ins},"param":{PARAM_GAIN},"value":1.0}}"#));
    assert!(ev.iter().any(|e| matches!(e, Event::PluginParam { .. })), "{ev:?}");
    let off = bass_energy(&mut engine, &mut h);
    assert!(off > baseline * 0.5, "désactivée : le réglage manuel redevient maître ({baseline} → {off})");
    // Le texte d'une valeur, tel que le plugin l'affiche, se lit sans rien changer.
    let ev = h.command_json(&format!(r#"{{"type":"get_plugin_param_text","insert":{ins},"param":{PARAM_GAIN},"value":0.5}}"#));
    assert!(ev.iter().any(|e| matches!(e, Event::PluginParamText { insert, param, .. } if *insert == ins && *param == PARAM_GAIN)), "{ev:?}");
    assert!(!error(&h.command_json(r#"{"type":"get_plugin_param_text","insert":9999,"param":0,"value":0.5}"#)).is_empty());
}

#[test]
fn plugin_parameter_curves_are_saved_with_the_project_and_follow_their_plugin_when_it_is_recreated() {
    let (_, mut h) = host();
    // Deux plugins sur la Basse : la courbe vise le deuxième (désigné par sa place dans la chaîne).
    add(&mut h, 2);
    let ev = add(&mut h, 2);
    let second = chain(&ev, "Basse")[1].id;
    let target = format!(r#"{{"kind":"insert","insert":{second},"param":{PARAM_GAIN}}}"#);
    let ev = h.command_json(&format!(r#"{{"type":"set_lane","node":2,"target":{target},"mode":"read","points":[{{"pos":[0,1],"value":0.25,"curve":"linear"}},{{"pos":[2,1],"value":1.5,"curve":"step"}}]}}"#));
    assert_eq!(node(&ev, "Basse").automation.len(), 1);
    let dir = saved(&mut h, "courbes");
    let text = std::fs::read_to_string(dir.join("P.daw/projet.json")).unwrap();
    assert!(text.contains(&format!("insert:1:{PARAM_GAIN}")), "la courbe est désignée par la place du plugin dans la chaîne");

    // Un autre hôte rouvre le projet : les plugins reçoivent de nouveaux identifiants, la courbe retrouve le deuxième.
    let (_, mut b) = host();
    let ev = b.command_json(&format!(r#"{{"type":"open_project","path":{:?}}}"#, dir.join("P.daw").to_string_lossy()));
    let c = chain(&ev, "Basse");
    assert_eq!(c.len(), 2);
    let lanes = &node(&ev, "Basse").automation;
    assert_eq!(lanes.len(), 1, "{ev:?}");
    assert_eq!(lanes[0].mode, "read");
    assert_eq!(lanes[0].points.len(), 2);
    assert_eq!((lanes[0].points[0].value, lanes[0].points[1].curve.as_str()), (0.25, "step"));
    match lanes[0].target {
        daw_ui::protocol::LaneTarget::Insert { insert, param } => assert_eq!((insert, param), (c[1].id, PARAM_GAIN)),
        t => panic!("{t:?}"),
    }
    // Relire puis réenregistrer ne perd rien : réécriture identique.
    let dir2 = std::env::temp_dir().join(format!("daw-plugins-courbes2-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir2);
    b.command_json(&format!(r#"{{"type":"save_project","path":{:?}}}"#, dir2.join("P").to_string_lossy()));
    let text2 = std::fs::read_to_string(dir2.join("P.daw/projet.json")).unwrap();
    assert!(text2.contains(&format!("insert:1:{PARAM_GAIN}")));
    // Retirer le plugin visé : sa courbe n'est plus enregistrée.
    b.command_json(&format!(r#"{{"type":"remove_plugin","insert":{}}}"#, c[1].id));
    let dir3 = std::env::temp_dir().join(format!("daw-plugins-courbes3-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir3);
    b.command_json(&format!(r#"{{"type":"save_project","path":{:?}}}"#, dir3.join("P").to_string_lossy()));
    assert!(!std::fs::read_to_string(dir3.join("P.daw/projet.json")).unwrap().contains("insert:"));
    // Un projet rouvert sans le plugin (résolveur vide) : la courbe est ignorée avec un avertissement, rien ne casse.
    let (_, handle) = Engine::new(SR, 64);
    let (session, meta) = demo_session(SR);
    let mut none = Host::new(handle, session, meta).unwrap();
    none.set_plugin_resolver(Box::new(|_| None));
    let ev = none.command_json(&format!(r#"{{"type":"open_project","path":{:?}}}"#, dir.join("P.daw").to_string_lossy()));
    let warnings: Vec<String> = ev.iter().filter_map(|e| if let Event::File { warnings, .. } = e { Some(warnings.clone()) } else { None }).flatten().collect();
    assert!(warnings.iter().any(|w| w.contains("courbe ignorée")), "{warnings:?}");
    assert!(node(&ev, "Basse").automation.is_empty());
    for d in [dir, dir2, dir3] {
        let _ = std::fs::remove_dir_all(d);
    }
}

#[test]
fn a_write_arm_on_a_plugin_parameter_follows_the_plugin_through_save_and_open() {
    let (_, mut h) = host();
    add(&mut h, 2);
    let ev = add(&mut h, 2);
    let second = chain(&ev, "Basse")[1].id;
    h.command_json(&format!(r#"{{"type":"set_write_mode","node":2,"target":{{"kind":"insert","insert":{second},"param":{PARAM_GAIN}}},"mode":"touch"}}"#));
    let dir = saved(&mut h, "armements");
    let (_, mut b) = host();
    let ev = b.command_json(&format!(r#"{{"type":"open_project","path":{:?}}}"#, dir.join("P.daw").to_string_lossy()));
    let c = chain(&ev, "Basse");
    let arms = &project(&ev).write_arms;
    assert_eq!(arms.len(), 1, "{ev:?}");
    match arms[0].target {
        daw_ui::protocol::LaneTarget::Insert { insert, param } => assert_eq!((insert, param, arms[0].mode.as_str()), (c[1].id, PARAM_GAIN, "touch")),
        t => panic!("{t:?}"),
    }
    // Sans le plugin (résolveur vide) : l'armement est ignoré avec un avertissement.
    let (_, handle) = Engine::new(SR, 64);
    let (session, meta) = demo_session(SR);
    let mut none = Host::new(handle, session, meta).unwrap();
    none.set_plugin_resolver(Box::new(|_| None));
    let ev = none.command_json(&format!(r#"{{"type":"open_project","path":{:?}}}"#, dir.join("P.daw").to_string_lossy()));
    assert!(project(&ev).write_arms.is_empty());
    let warnings: Vec<String> = ev.iter().filter_map(|e| if let Event::File { warnings, .. } = e { Some(warnings.clone()) } else { None }).flatten().collect();
    assert!(warnings.iter().any(|w| w.contains("armement")), "{warnings:?}");
    let _ = std::fs::remove_dir_all(dir);
}

/// Le plugin de test retarde la lecture de 64 échantillons (compensation de latence du moteur) : la prise rebouclée contient donc ce
/// retard de plus, et le clip doit être posé 64 échantillons plus tôt qu'avec la seule latence de la carte.
#[test]
fn a_take_recorded_while_a_plugin_delays_the_playback_is_placed_earlier_by_that_latency() {
    let dir = std::env::temp_dir().join(format!("daw-rec-plugin-latency-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    for with_plugin in [false, true] {
        let (mut engine, mut h) = host();
        h.set_audio_inputs(2);
        h.set_record_compensation(100);
        let ev = h.command_json(r#"{"type":"get_project"}"#);
        let (basse, claviers) = (node(&ev, "Basse").id, node(&ev, "Claviers").id);
        if with_plugin {
            let ev = h.command_json(&format!(r#"{{"type":"add_plugin","node":{claviers},"clap_id":"{}"}}"#, PLUGIN_ID.to_str().unwrap()));
            assert_eq!(project(&ev).latency_samples, 64);
        }
        h.command_json(&format!(r#"{{"type":"set_track_input","node":{basse},"input":{{"kind":"audio","channels":[1]}}}}"#));
        h.command_json(&format!(r#"{{"type":"set_node","node":{basse},"arm":true}}"#));
        let ev = h.command_json(&format!(r#"{{"type":"save_project","path":{:?}}}"#, dir.join(format!("P{with_plugin}")).to_string_lossy()));
        assert!(ev.iter().any(|e| matches!(e, Event::File { dirty: false, .. })), "{ev:?}");
        h.command_json(r#"{"type":"seek","pos":1000}"#);
        let (mut l, mut r) = (vec![0.0f32; 64], vec![0.0f32; 64]);
        let input = vec![0.1f32; 64];
        let mut feed = |engine: &mut Engine, blocks: usize| {
            for _ in 0..blocks {
                engine.process_inputs(&[&input], &mut l, &mut r);
            }
        };
        feed(&mut engine, 2);
        h.command_json(r#"{"type":"record"}"#);
        feed(&mut engine, 20);
        h.command_json(r#"{"type":"stop_record"}"#);
        let mut start = None;
        for _ in 0..400 {
            feed(&mut engine, 1);
            let ev = h.tick();
            if ev.iter().any(|e| matches!(e, Event::Recording { active: false, .. })) {
                let ev = h.command_json(r#"{"type":"get_project"}"#);
                start = node(&ev, "Basse").clips.iter().find_map(|c| match c {
                    daw_ui::protocol::ClipInfo::Audio { name, start, .. } if name.contains("prise") => Some(*start),
                    _ => None,
                });
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        let start = start.expect("un clip de prise est posé");
        // Départ de la prise à 1000 ; la carte compte 100 échantillons, les plugins 0 ou 64.
        assert_eq!(start, 1000 - 100 - if with_plugin { 64 } else { 0 }, "avec plugin : {with_plugin}");
    }
    let _ = std::fs::remove_dir_all(&dir);
}

fn sc_resolver() -> PluginResolver {
    let bundle = Arc::new(Bundle::from_static_entry(entry()).unwrap());
    Box::new(move |e| {
        let ids = [PLUGIN_ID.to_str().unwrap(), daw_test_plugin::SIDECHAIN_ID.to_str().unwrap()];
        ids.contains(&e.clap_id.as_str()).then(|| (clap_insert_factory(Arc::clone(&bundle), e.clap_id.clone()), PluginRef { clap_id: e.clap_id.clone(), vendor: "daw-perso".into(), name: "Plugin de test".into(), file: "test.clap".into() }))
    })
}

#[test]
fn a_plugin_side_chain_is_set_through_the_protocol_refused_when_invalid_and_saved_with_the_project() {
    let (mut engine, handle) = Engine::new(SR, 64);
    let (session, meta) = demo_session(SR);
    let mut h = Host::new(handle, session, meta).unwrap();
    h.set_plugin_resolver(sc_resolver());
    let p = h.command_json(r#"{"type":"get_project"}"#);
    let (drums, bass) = (node(&p, "Batterie").id, node(&p, "Basse").id);
    let ev = h.command_json(&format!(r#"{{"type":"add_plugin","node":{bass},"clap_id":"com.daw-perso.test-sidechain","file":"test.clap"}}"#));
    let ins = chain(&ev, "Basse")[0].clone();
    assert!(ins.has_sidechain && ins.sidechain.is_none(), "{ins:?}");
    let plain = chain(&add(&mut h, drums), "Batterie")[0].clone();
    assert!(!plain.has_sidechain, "le gain de test n'a pas de second port");
    // Une source valide.
    let ev = h.command_json(&format!(r#"{{"type":"set_insert_sidechain","insert":{},"source":{drums}}}"#, ins.id));
    assert_eq!(chain(&ev, "Basse")[0].sidechain, Some(drums));
    // Refus : la sortie principale, le nœud lui-même, un nœud inconnu, un effet inconnu ; une boucle (la batterie dépend de la basse).
    for (insert, src) in [(ins.id, 0u32), (ins.id, bass), (ins.id, 999), (777, drums)] {
        let ev = h.command_json(&format!(r#"{{"type":"set_insert_sidechain","insert":{insert},"source":{src}}}"#));
        assert!(ev.iter().any(|e| matches!(e, Event::Error { .. })), "insert {insert} source {src}");
    }
    let ev = h.command_json(&format!(r#"{{"type":"set_insert_sidechain","insert":{},"source":{bass}}}"#, plain.id));
    assert!(ev.iter().any(|e| matches!(e, Event::Error { .. })), "boucle refusée");
    assert_eq!(chain(&h.command_json(r#"{"type":"get_project"}"#), "Basse")[0].sidechain, Some(drums), "les refus n'ont rien changé");
    run(&mut engine, 4);
    // Enregistré et rouvert : la source revient.
    let dir = saved(&mut h, "sidechain");
    h.command_json(r#"{"type":"new_project"}"#);
    let ev = h.command_json(&format!(r#"{{"type":"open_project","path":{:?}}}"#, dir.join("P.daw").to_string_lossy()));
    assert!(!ev.iter().any(|e| matches!(e, Event::Error { .. })), "{ev:?}");
    let p = h.command_json(r#"{"type":"get_project"}"#);
    assert_eq!(chain(&p, "Basse")[0].sidechain, Some(node(&p, "Batterie").id));
    // Supprimer la source : la chaîne latérale disparaît ; annuler la rend.
    let drums = node(&p, "Batterie").id;
    let ev = h.command_json(&format!(r#"{{"type":"delete_track","node":{drums}}}"#));
    assert_eq!(chain(&ev, "Basse")[0].sidechain, None);
    let ev = h.command_json(r#"{"type":"undo"}"#);
    assert_eq!(chain(&ev, "Basse")[0].sidechain, Some(node(&ev, "Batterie").id));
    let _ = std::fs::remove_dir_all(&dir);
}
