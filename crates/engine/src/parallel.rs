//! Calcul des pistes en parallèle : un petit **groupe de fils de travail** qui aide le fil audio à calculer les nœuds indépendants d'un
//! même niveau du graphe (typiquement toutes les pistes, avec leurs plugins).
//!
//! Principes (voir `CLAUDE.md`, tranche 40) :
//! - **Le fil audio ne dépend jamais des fils de travail.** Il publie une tâche (un tableau de nœuds numérotés `0..count`), calcule
//!   lui-même sa part, puis **prend** ce que les autres n'ont pas encore pris. Si les fils de travail dorment ou sont retardés, il
//!   fait tout seul (vitesse du calcul sur un seul cœur, plus un faible surcoût) : aucune interblocage possible de ce côté.
//!   Seul cas d'attente : un fil de travail a **pris** un nœud et n'a pas fini ; le fil audio attend alors la fin de ce nœud.
//! - **Affectation stable** : un nœud est toujours calculé par le même fil tant que la répartition ne change pas (un plugin ne
//!   change donc pas de fil à chaque bloc : mémoire cache chaude, et les plugins qui s'attachent à un fil sont épargnés). Un autre
//!   fil ne le prend que s'il est en retard.
//! - **Aucune allocation, aucun verrou** dans le chemin du fil audio : compteurs atomiques seulement.
//! - **Résultat identique** quel que soit le nombre de fils (chaque nœud ne lit et n'écrit que ses propres données ; le routage et
//!   la somme restent séquentiels, dans un ordre fixe) : le rendu hors ligne est identique au bit près.
//!
//! **Protocole de publication** : une tâche est décrite dans un enregistrement à deux emplacements (numéro de tâche pair/impair) protégé
//! par un numéro de séquence (verrou de séquence) : un fil de travail en retard qui lit un enregistrement déjà réécrit le voit et
//! l'abandonne. Sans cela, un fil en retard pouvait mélanger l'ancienne tâche et la nouvelle et exécuter un nœud en double (défaut
//! trouvé par un test d'identité au bit près, voir `CLAUDE.md` tranche 40).
//!
//! Les fils de travail tournent en **attente active** tant que la lecture est en cours (le réveil d'un fil par le système coûte plus
//! cher que 1 ms) et se mettent en veille par courtes pauses quand plus rien n'est publié depuis `HOT_NS`.

#[cfg(not(target_arch = "wasm32"))]
mod imp {
    use std::sync::atomic::{AtomicBool, AtomicPtr, AtomicU64, AtomicU8, AtomicUsize, Ordering::*};
    use std::sync::Arc;
    use std::thread::JoinHandle;
    use std::time::{Duration, Instant};

    /// Nombre maximal de nœuds d'une tâche parallèle (au-delà : calcul séquentiel).
    pub const MAX_PAR: usize = 512;
    /// Les fils restent en attente active tant qu'une tâche a été publiée depuis moins de 50 ms.
    const HOT_NS: u64 = 50_000_000;

    pub type RunFn = unsafe fn(*mut (), usize);

    /// Une tâche : nombre de nœuds, fonction et contexte, sous un numéro de séquence (le numéro de la tâche quand l'enregistrement est
    /// complet et stable, `u64::MAX` pendant l'écriture).
    struct Slot {
        seq: AtomicU64,
        count: AtomicUsize,
        func: AtomicUsize,
        ctx: AtomicPtr<()>,
    }

    impl Slot {
        fn new() -> Slot {
            Slot { seq: AtomicU64::new(0), count: AtomicUsize::new(0), func: AtomicUsize::new(0), ctx: AtomicPtr::new(std::ptr::null_mut()) }
        }
    }

    /// Compteurs de diagnostic (lecture libre depuis n'importe quel fil) : combien de fois le fil audio a dû **attendre** un fil de
    /// travail après avoir fini sa part, et combien de temps. Une attente longue = un fil de travail retardé (préemption, plugin lent).
    #[derive(Default)]
    pub struct PoolStats {
        pub runs: AtomicU64,
        pub waits: AtomicU64,
        pub wait_total_ns: AtomicU64,
        pub wait_max_ns: AtomicU64,
        /// Attentes de plus de 100 µs.
        pub long_waits: AtomicU64,
    }

    struct Shared {
        stats: Arc<PoolStats>,
        /// Numéro de la tâche en cours (strictement croissant) ; publié en dernier, avec `Release`.
        epoch: AtomicU64,
        /// Description des deux dernières tâches (emplacement `numéro & 1`).
        slots: [Slot; 2],
        /// Fil prévu pour chaque nœud (0 = fil audio).
        assign: Vec<AtomicU8>,
        /// Dernière tâche qui a pris le nœud `k` (numéro de tâche) : une prise réussit une seule fois par tâche.
        claims: Vec<AtomicU64>,
        done: AtomicUsize,
        stop: AtomicBool,
        base: Instant,
        last_job: AtomicU64,
    }

    impl Shared {
        fn now_ns(&self) -> u64 {
            self.base.elapsed().as_nanos() as u64
        }

        /// Prend le nœud `k` pour la tâche `e` ; vrai pour un seul appelant.
        fn claim(&self, k: usize, e: u64) -> bool {
            let slot = &self.claims[k];
            let mut cur = slot.load(Relaxed);
            loop {
                if cur >= e {
                    return false;
                }
                match slot.compare_exchange_weak(cur, e, AcqRel, Relaxed) {
                    Ok(_) => return true,
                    Err(c) => cur = c,
                }
            }
        }

        /// Lit la description de la tâche `e` ; `None` si l'enregistrement a déjà été réécrit par une tâche plus récente.
        fn snapshot(&self, e: u64) -> Option<(usize, usize, *mut ())> {
            let slot = &self.slots[(e & 1) as usize];
            if slot.seq.load(Acquire) != e {
                return None;
            }
            let (count, func, ctx) = (slot.count.load(Relaxed), slot.func.load(Relaxed), slot.ctx.load(Relaxed));
            std::sync::atomic::fence(Acquire);
            (slot.seq.load(Relaxed) == e).then_some((count, func, ctx))
        }

        /// Fait ce qu'on peut de la tâche `e` : d'abord les nœuds prévus pour `me`, puis ceux que personne n'a pris.
        fn help(&self, me: usize, e: u64) {
            let Some((count, func, ctx)) = self.snapshot(e) else { return };
            let count = count.min(MAX_PAR);
            for pass in 0..2 {
                for k in 0..count {
                    if pass == 0 && self.assign[k].load(Relaxed) as usize != me {
                        continue;
                    }
                    if self.claim(k, e) {
                        // SAFETY: la description de la tâche `e` est complète (verrou de séquence) et `k < count` de cette tâche ;
                        // la prise a réussi, donc la tâche n'est pas terminée (le fil audio attend `done == count`) et ne sera
                        // pas remplacée ; `func` est celle passée à `run` ; chaque nœud n'est pris qu'une fois par tâche.
                        let f: RunFn = unsafe { std::mem::transmute::<usize, RunFn>(func) };
                        // Un plugin qui panique ne doit pas bloquer le fil audio : on compte le nœud comme terminé quand même.
                        let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| unsafe { f(ctx, k) }));
                        self.done.fetch_add(1, Release);
                    }
                }
            }
        }
    }

    /// Fils de travail du calcul parallèle. Se crée hors du fil audio (il lance des fils).
    pub struct Pool {
        shared: Arc<Shared>,
        workers: Vec<JoinHandle<()>>,
        threads: usize,
    }

    impl Pool {
        /// `threads` : nombre total de fils qui calculent, fil audio compris (≥ 1) ; `threads - 1` fils sont lancés.
        /// `on_start(numéro)` est appelé au début de chaque fil de travail (priorité temps réel, par exemple).
        pub fn new(threads: usize, on_start: Option<Arc<dyn Fn(usize) + Send + Sync>>) -> Pool {
            Self::build(threads, on_start, true)
        }

        /// Comme `new` mais sans lancer aucun fil : pour tester que le fil audio se suffit à lui-même.
        #[doc(hidden)]
        pub fn without_workers(threads: usize) -> Pool {
            Self::build(threads, None, false)
        }

        fn build(threads: usize, on_start: Option<Arc<dyn Fn(usize) + Send + Sync>>, spawn: bool) -> Pool {
            let threads = threads.clamp(1, 64);
            let shared = Arc::new(Shared {
                stats: Arc::new(PoolStats::default()),
                epoch: AtomicU64::new(0),
                slots: [Slot::new(), Slot::new()],
                assign: (0..MAX_PAR).map(|_| AtomicU8::new(0)).collect(),
                claims: (0..MAX_PAR).map(|_| AtomicU64::new(0)).collect(),
                done: AtomicUsize::new(0),
                stop: AtomicBool::new(false),
                base: Instant::now(),
                last_job: AtomicU64::new(0),
            });
            let mut workers = Vec::new();
            if spawn {
                for me in 1..threads {
                    let s = Arc::clone(&shared);
                    let hook = on_start.clone();
                    workers.push(
                        std::thread::Builder::new()
                            .name(format!("daw-calcul-{me}"))
                            .spawn(move || worker(s, me, hook))
                            .expect("fil de calcul"),
                    );
                }
            }
            Pool { shared, workers, threads }
        }

        /// Nombre de fils qui calculent (fil audio compris).
        pub fn threads(&self) -> usize {
            self.threads
        }

        /// Compteurs de diagnostic, qui survivent au groupe (à garder avant de confier le groupe au moteur).
        pub fn stats(&self) -> Arc<PoolStats> {
            Arc::clone(&self.shared.stats)
        }

        /// Exécute `f(ctx, k)` pour chaque `k` dans `0..count`, en parallèle, et rend la main quand **tout** est fini. `assign[k]` est le
        /// fil prévu pour `k` (0 = fil audio ; valeurs ≥ `threads` ramenées à 0). Ne doit être appelée que depuis le fil audio (un seul
        /// appelant à la fois). Sans allocation.
        ///
        /// # Safety
        /// `f` doit pouvoir être appelée depuis n'importe quel fil pour des `k` distincts **en même temps** avec ce `ctx`, et `ctx` doit
        /// rester valide jusqu'au retour de cette fonction.
        pub unsafe fn run(&self, count: usize, assign: &[u8], ctx: *mut (), f: RunFn) {
            let s = &*self.shared;
            if count == 0 {
                return;
            }
            if count > MAX_PAR || self.threads < 2 {
                for k in 0..count {
                    f(ctx, k);
                }
                return;
            }
            for k in 0..count {
                let a = assign.get(k).copied().unwrap_or(0);
                s.assign[k].store(if (a as usize) < self.threads { a } else { 0 }, Relaxed);
            }
            s.done.store(0, Relaxed);
            s.last_job.store(s.now_ns(), Relaxed);
            let e = s.epoch.load(Relaxed) + 1;
            let slot = &s.slots[(e & 1) as usize];
            slot.seq.store(u64::MAX, Relaxed); // en cours d'écriture : aucun lecteur ne l'accepte
            std::sync::atomic::fence(Release);
            slot.count.store(count, Relaxed);
            slot.func.store(f as usize, Relaxed);
            slot.ctx.store(ctx, Relaxed);
            slot.seq.store(e, Release); // description complète
            s.epoch.store(e, Release); // annonce la tâche aux fils de travail
            s.help(0, e);
            // Reste ce que d'autres fils ont pris et pas fini.
            s.stats.runs.fetch_add(1, Relaxed);
            if s.done.load(Acquire) < count {
                let t0 = Instant::now();
                while s.done.load(Acquire) < count {
                    std::hint::spin_loop();
                }
                let ns = t0.elapsed().as_nanos() as u64;
                s.stats.waits.fetch_add(1, Relaxed);
                s.stats.wait_total_ns.fetch_add(ns, Relaxed);
                s.stats.wait_max_ns.fetch_max(ns, Relaxed);
                if ns > 100_000 {
                    s.stats.long_waits.fetch_add(1, Relaxed);
                }
            }
        }
    }

    impl Drop for Pool {
        fn drop(&mut self) {
            self.shared.stop.store(true, Relaxed);
            for w in self.workers.drain(..) {
                let _ = w.join();
            }
        }
    }

    fn worker(s: Arc<Shared>, me: usize, on_start: Option<Arc<dyn Fn(usize) + Send + Sync>>) {
        if let Some(f) = on_start {
            f(me);
        }
        let mut seen = s.epoch.load(Acquire);
        while !s.stop.load(Relaxed) {
            let e = s.epoch.load(Acquire);
            if e != seen {
                seen = e;
                s.help(me, e);
                continue;
            }
            if s.now_ns().saturating_sub(s.last_job.load(Relaxed)) < HOT_NS {
                std::hint::spin_loop();
            } else {
                std::thread::sleep(Duration::from_micros(500));
            }
        }
    }
}

#[cfg(target_arch = "wasm32")]
mod imp {
    //! Sans fils (WebAssembly) : tout se calcule sur le fil appelant.
    pub const MAX_PAR: usize = 512;
    pub type RunFn = unsafe fn(*mut (), usize);
    #[derive(Default)]
    pub struct PoolStats;
    pub struct Pool;
    impl Pool {
        pub fn stats(&self) -> std::sync::Arc<PoolStats> {
            std::sync::Arc::new(PoolStats)
        }
        pub fn new(_: usize, _: Option<std::sync::Arc<dyn Fn(usize) + Send + Sync>>) -> Pool {
            Pool
        }
        pub fn threads(&self) -> usize {
            1
        }
        /// # Safety
        /// Voir la version native.
        pub unsafe fn run(&self, count: usize, _: &[u8], ctx: *mut (), f: RunFn) {
            for k in 0..count {
                f(ctx, k);
            }
        }
    }
}

pub use imp::{Pool, PoolStats};
#[allow(unused_imports)]
pub use imp::{RunFn, MAX_PAR};

#[cfg(all(test, not(target_arch = "wasm32")))]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;
    use std::time::{Duration, Instant};

    struct Ctx {
        hits: Vec<AtomicUsize>,
        total: AtomicUsize,
    }
    unsafe fn bump(ctx: *mut (), k: usize) {
        let c = &*(ctx as *const Ctx);
        c.hits[k].fetch_add(1, Ordering::Relaxed);
        c.total.fetch_add(1, Ordering::Relaxed);
    }

    fn ctx(n: usize) -> Ctx {
        Ctx { hits: (0..n).map(|_| AtomicUsize::new(0)).collect(), total: AtomicUsize::new(0) }
    }

    #[test]
    fn every_item_runs_exactly_once_per_run_whatever_the_thread_count() {
        for threads in [1, 2, 3, 4] {
            let pool = Pool::new(threads, None);
            let c = ctx(40);
            let assign: Vec<u8> = (0..40).map(|k| (k % threads) as u8).collect();
            for round in 1..=300 {
                unsafe { pool.run(40, &assign, &c as *const Ctx as *mut (), bump) };
                assert_eq!(c.total.load(Ordering::Relaxed), 40 * round, "{threads} fils, tour {round}");
            }
            assert!(c.hits.iter().all(|h| h.load(Ordering::Relaxed) == 300));
        }
    }

    /// Régression : des tâches de tailles très différentes à la suite (un fil de travail en retard lisait la taille de la tâche
    /// suivante avec le numéro de la précédente et exécutait un nœud en double). Chaque nœud de chaque tâche tourne **exactement une fois**.
    #[test]
    fn every_item_of_every_round_runs_exactly_once_even_when_sizes_change_all_the_time() {
        struct Round {
            hits: Vec<AtomicUsize>,
        }
        unsafe fn mark(ctx: *mut (), k: usize) {
            let r = &*(ctx as *const Round);
            r.hits[k].fetch_add(1, Ordering::SeqCst);
            // un peu de travail : les fils en retard existent vraiment
            std::hint::black_box((0..50).fold(0u64, |a, b| a.wrapping_add(b * 7)));
        }
        let sizes = [40usize, 3, 33, 2, 60, 7, 2, 50, 5, 31];
        for threads in [2usize, 3, 4, 6] {
            let pool = Pool::new(threads, None);
            for round in 0..6_000usize {
                let n = sizes[round % sizes.len()];
                let r = Round { hits: (0..64).map(|_| AtomicUsize::new(0)).collect() };
                let assign: Vec<u8> = (0..n).map(|k| ((k + round) % threads) as u8).collect();
                unsafe { pool.run(n, &assign, &r as *const Round as *mut (), mark) };
                for k in 0..64 {
                    let want = usize::from(k < n);
                    assert_eq!(r.hits[k].load(Ordering::SeqCst), want, "{threads} fils, tour {round}, nœud {k} sur {n}");
                }
            }
        }
    }

    #[test]
    fn the_wait_counters_count_runs_and_long_waits() {
        unsafe fn slow_for_workers(ctx: *mut (), k: usize) {
            let c = &*(ctx as *const Ctx);
            c.total.fetch_add(1, Ordering::Relaxed);
            if k == 1 {
                std::thread::sleep(Duration::from_millis(2)); // un nœud long : celui qu'un fil de travail aura pris
            }
        }
        let pool = Pool::new(2, None);
        let stats = pool.stats();
        let c = ctx(2);
        for _ in 0..20 {
            // nœud 1 prévu pour le fil de travail ; le fil audio prend le nœud 0 puis attend le nœud 1 s'il a été pris
            unsafe { pool.run(2, &[0, 1], &c as *const Ctx as *mut (), slow_for_workers) };
        }
        assert_eq!(stats.runs.load(Ordering::Relaxed), 20);
        assert_eq!(c.total.load(Ordering::Relaxed), 40);
        // Selon qui a pris le nœud lent, le fil audio a attendu ou l'a fait lui-même : dans tous les cas le total d'attente est cohérent.
        let (waits, total, max) = (stats.waits.load(Ordering::Relaxed), stats.wait_total_ns.load(Ordering::Relaxed), stats.wait_max_ns.load(Ordering::Relaxed));
        assert!(waits <= 20 && max <= total && (waits == 0) == (total == 0));
    }

    #[test]
    fn the_calling_thread_finishes_the_job_alone_when_no_worker_runs() {
        let pool = Pool::without_workers(4);
        let c = ctx(16);
        let assign = [1u8, 2, 3, 1, 2, 3, 1, 2, 3, 0, 0, 0, 1, 2, 3, 0]; // presque tout « prévu » pour des fils absents
        unsafe { pool.run(16, &assign, &c as *const Ctx as *mut (), bump) };
        assert!(c.hits.iter().all(|h| h.load(Ordering::Relaxed) == 1), "le fil appelant prend ce que personne ne prend");
    }

    #[test]
    fn more_items_than_the_limit_and_empty_runs_are_handled() {
        let pool = Pool::new(3, None);
        let c = ctx(MAX_PAR + 10);
        unsafe { pool.run(MAX_PAR + 10, &[], &c as *const Ctx as *mut (), bump) };
        assert_eq!(c.total.load(Ordering::Relaxed), MAX_PAR + 10);
        unsafe { pool.run(0, &[], &c as *const Ctx as *mut (), bump) };
        assert_eq!(c.total.load(Ordering::Relaxed), MAX_PAR + 10);
    }

    #[test]
    fn a_panicking_item_does_not_block_the_caller() {
        unsafe fn boom(ctx: *mut (), k: usize) {
            let c = &*(ctx as *const Ctx);
            c.total.fetch_add(1, Ordering::Relaxed);
            if k % 5 == 0 {
                panic!("plugin défaillant");
            }
        }
        let prev = std::panic::take_hook();
        std::panic::set_hook(Box::new(|_| {})); // pas de bruit : le panic est voulu
        let pool = Pool::new(3, None);
        let c = ctx(20);
        unsafe { pool.run(20, &[0u8; 20], &c as *const Ctx as *mut (), boom) };
        assert_eq!(c.total.load(Ordering::Relaxed), 20);
        std::panic::set_hook(prev);
    }

    #[test]
    fn workers_stop_when_the_pool_is_dropped() {
        let started = Arc::new(AtomicUsize::new(0));
        let s = Arc::clone(&started);
        let hook: Arc<dyn Fn(usize) + Send + Sync> = Arc::new(move |_| {
            s.fetch_add(1, Ordering::SeqCst);
        });
        let pool = Pool::new(4, Some(hook));
        assert_eq!(pool.threads(), 4);
        let t0 = Instant::now();
        while started.load(Ordering::SeqCst) < 3 && t0.elapsed() < Duration::from_secs(5) {
            std::thread::yield_now();
        }
        assert_eq!(started.load(Ordering::SeqCst), 3, "un crochet par fil de travail (3 fils en plus du fil audio)");
        drop(pool); // doit rendre la main (les fils de travail s'arrêtent)
    }
}
