//! Serveur web local des retours de monitoring : chaque retour (bus qui sort sur la carte son) a une adresse secrète, un code QR, et une
//! page de tablette qui n'affiche que **son** mix (le niveau de l'envoi de chaque piste vers ce retour) et ne permet de régler que ça.
//!
//! Le serveur n'existe que si l'utilisateur le démarre. Il tourne sur ses propres fils ; il **ne touche jamais au moteur ni à la session** :
//! il lit un instantané JSON publié par l'hôte (`Monitor::publish`) et dépose les demandes de réglage dans une file que l'hôte vide à chaque
//! `tick` (`Monitor::take_requests`). L'hôte reste donc le seul à modifier le projet (historique, état « modifié », moteur).

use std::collections::{HashMap, VecDeque};
use std::hash::{BuildHasher, Hasher};
use std::io::Read;
use std::net::{IpAddr, UdpSocket};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use tiny_http::{Header, Method, Response, Server};

/// Page de la tablette (une seule page autonome : aucune ressource externe).
const PAGE: &str = include_str!("monitor.html");
/// Taille maximale du corps d'une requête de réglage.
const MAX_BODY: u64 = 1024;
/// Nombre de fils qui répondent aux requêtes (un client lent ne bloque pas les autres).
const WORKERS: usize = 4;

/// Une demande de réglage venue d'une tablette. Elle ne porte que sur le mix de **son** retour `ret`.
#[derive(Debug, Clone, PartialEq)]
pub enum SetRequest {
    /// Niveau (en dB) de l'envoi de `node` vers le retour.
    Level { ret: u32, node: u32, db: f64 },
    /// Coupe / solo de l'envoi de `node` vers le retour (dans ce retour seulement).
    Flags { ret: u32, node: u32, mute: Option<bool>, solo: Option<bool> },
    /// Mute, solo et/ou décalage de niveau (dB, écarts conservés) de tous les envois des membres d'un groupe vers le retour.
    Group { ret: u32, group: u32, mute: Option<bool>, solo: Option<bool>, delta_db: Option<f64> },
    /// Place le décalage de niveau du groupe dans ce retour à `db` (absolu, −12 à +12) : le curseur de la tablette reste à sa place.
    GroupOffset { ret: u32, group: u32, db: f64 },
    /// Télécommande : seulement acceptée d'une page dont le retour a le rôle « transport » (voir `Role`).
    Remote { ret: u32, cmd: RemoteCmd },
    /// Régie : réglage d'une piste ou d'un bus quelconque (seulement avec le droit `ROLE_MIXER`).
    Mix { ret: u32, cmd: MixCmd },
    /// Régie : muet, solo et/ou niveau (décalage absolu −12 à +12 dB) d'un groupe de **pistes** du projet (seulement avec le droit `ROLE_MIXER`).
    MixGroup { ret: u32, group: u32, mute: Option<bool>, solo: Option<bool>, offset_db: Option<f64> },
}

/// Un réglage de régie sur un nœud (piste, bus, retour ou sortie principale). Chaque champ est facultatif ; au moins un est présent.
#[derive(Debug, Clone, PartialEq)]
pub struct MixCmd {
    pub node: u32,
    /// Niveau en dB (borné à −60 / +6 par l'hôte ; −60 = coupé).
    pub db: Option<f64>,
    /// Panoramique de −1 (gauche) à +1.
    pub pan: Option<f64>,
    pub mute: Option<bool>,
    pub solo: Option<bool>,
    /// Armement de la piste pour l'enregistrement (refusé par l'hôte pour un bus, et pendant une prise).
    pub arm: Option<bool>,
}

/// Lit le corps d'un réglage de régie. `None` si le nœud manque, si une valeur n'est pas du bon type ou pas finie, ou s'il n'y a rien à régler.
fn parse_mix(v: &serde_json::Value) -> Option<MixCmd> {
    let node = v["node"].as_u64().filter(|n| *n <= u32::MAX as u64)? as u32;
    let flag = |k: &str| match &v[k] {
        serde_json::Value::Null => Some(None),
        serde_json::Value::Bool(b) => Some(Some(*b)),
        _ => None,
    };
    let num = |k: &str| match &v[k] {
        serde_json::Value::Null => Some(None),
        n => n.as_f64().filter(|x| x.is_finite()).map(Some),
    };
    let cmd = MixCmd { node, db: num("db")?, pan: num("pan")?, mute: flag("mute")?, solo: flag("solo")?, arm: flag("arm")? };
    (cmd.db.is_some() || cmd.pan.is_some() || cmd.mute.is_some() || cmd.solo.is_some() || cmd.arm.is_some()).then_some(cmd)
}

/// Lit le corps d'un réglage de groupe de la régie : `group` plus au moins un de `mute`, `solo`, `offset_db`.
fn parse_mix_group(ret: u32, v: &serde_json::Value) -> Option<SetRequest> {
    let group = v["group"].as_u64().filter(|n| *n <= u32::MAX as u64)? as u32;
    let flag = |k: &str| match &v[k] {
        serde_json::Value::Null => Some(None),
        serde_json::Value::Bool(b) => Some(Some(*b)),
        _ => None,
    };
    let offset_db = match &v["offset_db"] {
        serde_json::Value::Null => None,
        n => Some(n.as_f64().filter(|x| x.is_finite())?.clamp(-12.0, 12.0)),
    };
    let (mute, solo) = (flag("mute")?, flag("solo")?);
    (mute.is_some() || solo.is_some() || offset_db.is_some()).then_some(SetRequest::MixGroup { ret, group, mute, solo, offset_db })
}

/// Droits d'une adresse, en bits qui se combinent (le rôle est un **ensemble de droits** du jeton, choisi par l'utilisateur dans la fenêtre du QR) :
/// toute adresse règle **son** retour (rôle 0) ; `ROLE_TRANSPORT` ajoute la télécommande (transport, sections, pré-roll) ; `ROLE_MIXER` ajoute la
/// régie (niveau, panoramique, muet, solo, armement de **toutes** les pistes, avec leurs vumètres). 3 = musicien + transport + régie. Une page qui n'a
/// pas le droit reçoit 403.
pub const ROLE_MUSICIAN: u8 = 0;
pub const ROLE_TRANSPORT: u8 = 1;
pub const ROLE_MIXER: u8 = 2;
/// Tous les droits connus : un rôle au-delà est refusé.
pub const ROLE_ALL: u8 = ROLE_TRANSPORT | ROLE_MIXER;

/// Une commande de télécommande. Elle passe ensuite par les commandes ordinaires de l'hôte (mêmes refus, mêmes garde-fous).
#[derive(Debug, Clone, PartialEq)]
pub enum RemoteCmd {
    /// Lecture / pause.
    Toggle,
    /// Arrêt (arrête aussi un enregistrement).
    Stop,
    /// Retour au début du projet.
    Start,
    /// Démarre l'enregistrement des pistes armées.
    Record,
    /// Va au début de la section d'indice `n` de la forme.
    Section(u32),
    /// Pré-roll en mesures (0 à 8).
    PreRoll(u32),
    /// Métronome allumé ou éteint.
    Click(bool),
    /// Le pré-roll s'applique-t-il aussi à la lecture normale.
    PlayPreRoll(bool),
}

/// Lit le corps d'une commande de télécommande. `None` si ce n'est pas une commande connue et valide.
fn parse_remote(v: &serde_json::Value) -> Option<RemoteCmd> {
    let n = |k: &str| v[k].as_u64().filter(|x| *x <= 100_000).map(|x| x as u32);
    Some(match v["cmd"].as_str()? {
        "toggle" => RemoteCmd::Toggle,
        "stop" => RemoteCmd::Stop,
        "start" => RemoteCmd::Start,
        "record" => RemoteCmd::Record,
        "section" => RemoteCmd::Section(n("index")?),
        "preroll" => RemoteCmd::PreRoll(n("bars").filter(|b| *b <= 8)?),
        "click" => RemoteCmd::Click(v["on"].as_bool()?),
        "playpreroll" => RemoteCmd::PlayPreRoll(v["on"].as_bool()?),
        _ => return None,
    })
}

/// Lit le corps d'une requête de réglage. `None` si ce n'est pas une demande valide.
fn parse_request(ret: u32, v: &serde_json::Value) -> Option<SetRequest> {
    let id = |k: &str| v[k].as_u64().map(|n| n.min(u32::MAX as u64) as u32);
    let flag = |k: &str| match &v[k] {
        serde_json::Value::Null => Some(None),
        serde_json::Value::Bool(b) => Some(Some(*b)),
        _ => None,
    };
    let finite = |k: &str| v[k].as_f64().filter(|x| x.is_finite());
    let (mute, solo) = (flag("mute")?, flag("solo")?);
    if let Some(group) = id("group") {
        if let Some(db) = finite("offset_db") {
            return Some(SetRequest::GroupOffset { ret, group, db: db.clamp(-12.0, 12.0) });
        }
        let delta_db = finite("delta_db").map(|d| d.clamp(-24.0, 24.0));
        if mute.is_none() && solo.is_none() && delta_db.is_none() {
            return None;
        }
        return Some(SetRequest::Group { ret, group, mute, solo, delta_db });
    }
    let node = id("node")?;
    if let Some(db) = finite("db") {
        return Some(SetRequest::Level { ret, node, db });
    }
    if mute.is_none() && solo.is_none() {
        return None;
    }
    Some(SetRequest::Flags { ret, node, mute, solo })
}

/// Nombre d'entrées gardées dans le journal des commandes reçues.
const JOURNAL_LEN: usize = 300;
/// Demandes de réglage d'une adresse inconnue tolérées par minute et par client avant de répondre 429 (une page périmée qui relit son état
/// toutes les 400 ms en fait 150 par minute : elle ne doit pas être bloquée, un balayage d'adresses oui).
const MISSES_PER_MINUTE: u32 = 300;

/// Seau à jetons : `burst` demandes d'un coup, puis `per_sec` par seconde.
#[derive(Debug, Clone, Copy)]
struct Bucket {
    tokens: f64,
    last: Instant,
}

impl Bucket {
    fn take(&mut self, now: Instant, burst: f64, per_sec: f64) -> bool {
        self.tokens = (self.tokens + now.duration_since(self.last).as_secs_f64() * per_sec).min(burst);
        self.last = now;
        if self.tokens >= 1.0 {
            self.tokens -= 1.0;
            true
        } else {
            false
        }
    }
}

/// Limites de débit des routes qui commandent (par adresse). Le transport est lent par nature ; les faders envoient en continu (la page groupe
/// ses envois toutes les 60 ms, soit ~17 par seconde).
fn limits(route: &str) -> (f64, f64) {
    match route {
        "cmd" => (8.0, 2.0),
        _ => (60.0, 40.0),
    }
}

/// Une commande reçue, pour le journal (ce que le serveur a vu, pas ce que l'hôte en a fait).
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct JournalEntry {
    /// Instant de réception, en millisecondes depuis 1970.
    pub t_ms: u64,
    /// Adresse du client.
    pub ip: String,
    /// Retour dont l'adresse a été utilisée (`None` : adresse inconnue).
    pub ret: Option<u32>,
    /// `cmd` (télécommande), `mix` (régie), `set` (retour) ; `?` pour une adresse inconnue.
    pub route: String,
    /// La commande lue (jamais le corps brut) ou la raison.
    pub what: String,
    /// Code de la réponse : 200 acceptée, 400 invalide, 403 droit manquant, 404 adresse inconnue, 429 trop de demandes.
    pub status: u16,
}

#[derive(Default)]
struct Shared {
    /// jeton → (nœud du retour, instantané JSON de son mix, rôle).
    returns: HashMap<String, (u32, String, u8)>,
    /// (jeton, route) → seau à jetons.
    buckets: HashMap<(String, &'static str), Bucket>,
    /// client → (début de la minute, demandes à une adresse inconnue).
    misses: HashMap<IpAddr, (Instant, u32)>,
    journal: VecDeque<JournalEntry>,
}

impl Shared {
    fn log(&mut self, ip: Option<IpAddr>, ret: Option<u32>, route: &str, what: String, status: u16) {
        let t_ms = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0);
        if self.journal.len() >= JOURNAL_LEN {
            self.journal.pop_front();
        }
        self.journal.push_back(JournalEntry { t_ms, ip: ip.map_or_else(|| "?".into(), |a| a.to_string()), ret, route: route.into(), what, status });
    }

    /// Compte une demande à une adresse inconnue ; `false` quand ce client en fait trop. Seule la première de chaque minute entre dans le journal.
    fn miss(&mut self, ip: Option<IpAddr>, what: &str) -> bool {
        let Some(ip) = ip else { return true };
        let now = Instant::now();
        let e = self.misses.entry(ip).or_insert((now, 0));
        if now.duration_since(e.0).as_secs() >= 60 {
            *e = (now, 0);
        }
        e.1 += 1;
        let (count, over) = (e.1, e.1 > MISSES_PER_MINUTE);
        if self.misses.len() > 1024 {
            self.misses.retain(|_, (t, _)| now.duration_since(*t).as_secs() < 60);
        }
        if count == 1 || count == MISSES_PER_MINUTE + 1 {
            self.log(Some(ip), None, "?", format!("{what}{}", if over { " (trop de demandes : bloqué une minute)" } else { "" }), if over { 429 } else { 404 });
        }
        !over
    }

    /// Une commande de l'adresse `token` sur la route `route` est-elle dans la limite de débit ?
    fn allow(&mut self, token: &str, route: &'static str) -> bool {
        let (burst, per_sec) = limits(route);
        let now = Instant::now();
        if self.buckets.len() > 256 {
            self.buckets.retain(|_, b| now.duration_since(b.last).as_secs() < 60);
        }
        self.buckets.entry((token.to_owned(), route)).or_insert(Bucket { tokens: burst, last: now }).take(now, burst, per_sec)
    }
}

pub struct Monitor {
    server: Arc<Server>,
    workers: Vec<JoinHandle<()>>,
    shared: Arc<Mutex<Shared>>,
    rx: Receiver<SetRequest>,
    /// Jeton de chaque retour (stable tant que le serveur tourne et que le projet ne change pas).
    tokens: HashMap<u32, String>,
    port: u16,
    ip: String,
}

impl Monitor {
    /// Démarre le serveur. `local_only` : n'écoute que cette machine (tests) ; sinon tout le réseau local. `port` 0 : un port libre ;
    /// sinon ce port, ou le premier libre parmi les 20 suivants.
    pub fn start(port: u16, local_only: bool) -> Result<Monitor, String> {
        let host = if local_only { "127.0.0.1" } else { "0.0.0.0" };
        let mut last = String::new();
        let mut server = None;
        for p in (0..if port == 0 { 1 } else { 20 }).map(|k| if port == 0 { 0 } else { port.saturating_add(k) }) {
            match Server::http(format!("{host}:{p}")) {
                Ok(s) => {
                    server = Some(s);
                    break;
                }
                Err(e) => last = e.to_string(),
            }
        }
        let server = Arc::new(server.ok_or_else(|| format!("impossible d'ouvrir le serveur : {last}"))?);
        let port = server.server_addr().to_ip().map(|a| a.port()).ok_or("adresse du serveur inconnue")?;
        let shared = Arc::new(Mutex::new(Shared::default()));
        let (tx, rx) = mpsc::channel();
        let workers = (0..WORKERS)
            .map(|_| {
                let (server, shared, tx) = (server.clone(), shared.clone(), tx.clone());
                std::thread::Builder::new().name("monitoring-web".into()).spawn(move || serve(server, shared, tx)).expect("fil du serveur")
            })
            .collect();
        let ip = if local_only { "127.0.0.1".to_string() } else { local_ip() };
        Ok(Monitor { server, workers, shared, rx, tokens: HashMap::new(), port, ip })
    }

    pub fn port(&self) -> u16 {
        self.port
    }

    /// Adresse de la page d'un retour (nécessite que `sync_returns` l'ait connu).
    pub fn url(&self, ret: u32) -> Option<String> {
        self.tokens.get(&ret).map(|t| format!("http://{}:{}/r/{t}", self.ip, self.port))
    }

    /// Code QR (SVG) d'une adresse.
    pub fn qr_svg(url: &str) -> String {
        use qrcode::render::svg;
        match qrcode::QrCode::new(url.as_bytes()) {
            Ok(code) => code.render::<svg::Color>().min_dimensions(200, 200).quiet_zone(true).dark_color(svg::Color("#000000")).light_color(svg::Color("#ffffff")).build(),
            Err(_) => String::new(),
        }
    }

    /// Met à jour la liste des retours et leurs jetons (donnés par l'hôte, qui les garde dans le projet : l'adresse d'un retour ne change
    /// pas d'une session à l'autre). Les retours disparus sont retirés. Rend `true` si quelque chose a changé.
    pub fn sync_returns(&mut self, returns: &[(u32, String)]) -> bool {
        let before = self.tokens.clone();
        self.tokens.retain(|id, _| returns.iter().any(|(r, _)| r == id));
        for (id, token) in returns {
            self.tokens.insert(*id, token.clone());
        }
        // Une adresse qui n'est plus celle d'un retour (retour supprimé, adresse renouvelée) cesse de répondre tout de suite.
        let live: Vec<&String> = self.tokens.values().collect();
        self.shared.lock().unwrap().returns.retain(|token, _| live.contains(&token));
        self.tokens != before
    }

    /// Oublie tous les jetons (un autre projet : les numéros de nœuds ne désignent plus les mêmes retours).
    pub fn forget_tokens(&mut self) {
        self.tokens.clear();
        self.shared.lock().unwrap().returns.clear();
    }

    /// Publie l'instantané JSON du mix d'un retour (ce que la tablette lira).
    pub fn publish(&mut self, ret: u32, json: String, role: u8) {
        if let Some(token) = self.tokens.get(&ret) {
            self.shared.lock().unwrap().returns.insert(token.clone(), (ret, json, role));
        }
    }

    /// Le journal des commandes reçues (les plus récentes en dernier), au plus `JOURNAL_LEN`.
    pub fn journal(&self) -> Vec<JournalEntry> {
        self.shared.lock().unwrap().journal.iter().cloned().collect()
    }

    /// Les demandes de réglage reçues depuis le dernier appel.
    pub fn take_requests(&mut self) -> Vec<SetRequest> {
        self.rx.try_iter().collect()
    }
}

impl Drop for Monitor {
    fn drop(&mut self) {
        for _ in 0..self.workers.len() {
            self.server.unblock();
        }
        for w in self.workers.drain(..) {
            let _ = w.join();
        }
    }
}

fn header(name: &str, value: &str) -> Header {
    Header::from_bytes(name.as_bytes(), value.as_bytes()).expect("en-tête valide")
}

fn json_ok() -> Response<std::io::Cursor<Vec<u8>>> {
    Response::from_string("{\"ok\":true}").with_header(header("Content-Type", "application/json"))
}

/// Une requête POST de réglage ou de commande. Ordre des contrôles : adresse connue, limite de débit, droit, corps. Chaque issue entre au journal.
fn post(req: &mut tiny_http::Request, shared: &Mutex<Shared>, tx: &Sender<SetRequest>, ip: Option<IpAddr>, token: &str, route: &'static str) -> Response<std::io::Cursor<Vec<u8>>> {
    let known = shared.lock().unwrap().returns.get(token).map(|(ret, _, role)| (*ret, *role));
    let Some((ret, role)) = known else {
        return if shared.lock().unwrap().miss(ip, &format!("POST {route} sur une adresse inconnue")) { not_found() } else { too_many() };
    };
    if !shared.lock().unwrap().allow(token, route) {
        shared.lock().unwrap().log(ip, Some(ret), route, "trop de demandes".into(), 429);
        return too_many();
    }
    let needed = match route {
        "cmd" => ROLE_TRANSPORT,
        "mix" => ROLE_MIXER,
        _ => 0,
    };
    if role & needed != needed {
        shared.lock().unwrap().log(ip, Some(ret), route, "droit manquant".into(), 403);
        let what = if route == "cmd" { "de télécommande" } else { "la régie" };
        return Response::from_string(format!("{{\"ok\":false,\"error\":\"cette adresse ne permet pas {what}\"}}")).with_status_code(403);
    }
    let mut body = String::new();
    let value = req.as_reader().take(MAX_BODY).read_to_string(&mut body).ok().and_then(|_| serde_json::from_str::<serde_json::Value>(&body).ok());
    let parsed = value.and_then(|v| match route {
        "cmd" => parse_remote(&v).map(|cmd| SetRequest::Remote { ret, cmd }),
        "mix" if v["group"].is_u64() => parse_mix_group(ret, &v),
        "mix" => parse_mix(&v).map(|cmd| SetRequest::Mix { ret, cmd }),
        _ => parse_request(ret, &v),
    });
    match parsed {
        Some(r) => {
            // Le journal garde la commande lue (jamais le corps brut).
            shared.lock().unwrap().log(ip, Some(ret), route, format!("{r:?}"), 200);
            let _ = tx.send(r);
            json_ok()
        }
        None => {
            shared.lock().unwrap().log(ip, Some(ret), route, "corps invalide".into(), 400);
            Response::from_string("{\"ok\":false}").with_status_code(400)
        }
    }
}

fn too_many() -> Response<std::io::Cursor<Vec<u8>>> {
    Response::from_string("{\"ok\":false,\"error\":\"trop de demandes\"}").with_status_code(429).with_header(header("Retry-After", "1"))
}

fn serve(server: Arc<Server>, shared: Arc<Mutex<Shared>>, tx: Sender<SetRequest>) {
    while let Ok(mut req) = server.recv() {
        let url = req.url().to_string();
        let path = url.split('?').next().unwrap_or("");
        let parts: Vec<&str> = path.trim_matches('/').split('/').collect();
        let method = req.method().clone();
        let ip = req.remote_addr().map(|a| a.ip());
        let known = |token: &str| shared.lock().unwrap().returns.get(token).map(|(ret, json, role)| (*ret, json.clone(), *role));
        let unknown = |what: &str| if shared.lock().unwrap().miss(ip, what) { not_found() } else { too_many() };
        let reply = match (&method, parts.as_slice()) {
            (Method::Get, ["r", token]) => match known(token) {
                Some(_) => Response::from_string(PAGE).with_header(header("Content-Type", "text/html; charset=utf-8")),
                None => unknown("GET page d'une adresse inconnue"),
            },
            (Method::Get, ["r", token, "state"]) => match known(token) {
                Some((_, json, _)) => Response::from_string(json).with_header(header("Content-Type", "application/json; charset=utf-8")),
                None => unknown("GET état d'une adresse inconnue"),
            },
            (Method::Post, ["r", token, "cmd"]) => post(&mut req, &shared, &tx, ip, token, "cmd"),
            (Method::Post, ["r", token, "mix"]) => post(&mut req, &shared, &tx, ip, token, "mix"),
            (Method::Post, ["r", token, "set"]) => post(&mut req, &shared, &tx, ip, token, "set"),
            _ => unknown("chemin inconnu"),
        };
        let _ = req.respond(reply.with_header(header("Cache-Control", "no-store")).with_header(header("X-Content-Type-Options", "nosniff")));
    }
}

fn not_found() -> Response<std::io::Cursor<Vec<u8>>> {
    Response::from_string("introuvable").with_status_code(404)
}

/// Jeton secret de 128 bits : deux tirages du hachage aléatoire du système (graine fournie par l'OS), mêlés à l'heure.
pub fn new_token(id: u32) -> String {
    let nanos = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0);
    let part = |salt: u64| {
        let mut h = std::collections::hash_map::RandomState::new().build_hasher();
        h.write_u128(nanos);
        h.write_u32(id);
        h.write_u64(salt);
        h.finish()
    };
    format!("{:016x}{:016x}", part(1), part(2))
}

/// Adresse de cette machine sur le réseau local (celle par laquelle sortirait un paquet vers l'extérieur ; aucun paquet n'est envoyé).
fn local_ip() -> String {
    UdpSocket::bind("0.0.0.0:0")
        .and_then(|s| s.connect("192.0.2.1:9").map(|_| s))
        .and_then(|s| s.local_addr())
        .map(|a| a.ip().to_string())
        .unwrap_or_else(|_| "127.0.0.1".into())
}
