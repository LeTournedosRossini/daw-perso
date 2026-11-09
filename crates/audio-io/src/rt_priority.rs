//! Priorité « temps réel » du thread courant, pour les fils de calcul des pistes (voir `engine::Pool`).
//!
//! Windows : priorité maximale du fil et inscription au service **MMCSS** (classe « Pro Audio »), comme le font les autres stations
//! audio : c'est ce qui empêche le système de retarder un fil de calcul pendant que le fil audio l'attend. Ailleurs : sans effet pour
//! l'instant (Linux demanderait `SCHED_FIFO` via rtkit, macOS les « workgroups » : voir `CLAUDE.md`).

/// Élève le fil courant ; rend vrai si le système a accepté.
pub fn promote_current_thread() -> bool {
    imp::promote()
}

#[cfg(windows)]
mod imp {
    #[link(name = "kernel32")]
    extern "system" {
        fn GetCurrentThread() -> isize;
        fn SetThreadPriority(thread: isize, priority: i32) -> i32;
    }
    #[link(name = "avrt")]
    extern "system" {
        fn AvSetMmThreadCharacteristicsW(task: *const u16, index: *mut u32) -> isize;
    }

    const THREAD_PRIORITY_TIME_CRITICAL: i32 = 15;

    pub fn promote() -> bool {
        let task: Vec<u16> = "Pro Audio".encode_utf16().chain(std::iter::once(0)).collect();
        let mut index = 0u32;
        // SAFETY: appels Win32 sans effet de bord mémoire : `task` est une chaîne UTF-16 terminée par 0 qui vit pendant l'appel,
        // `index` un entier valide ; le pseudo-handle de `GetCurrentThread` est toujours valide pour le fil appelant.
        unsafe {
            let prio = SetThreadPriority(GetCurrentThread(), THREAD_PRIORITY_TIME_CRITICAL) != 0;
            let mmcss = AvSetMmThreadCharacteristicsW(task.as_ptr(), &mut index) != 0;
            prio || mmcss
        }
    }
}

#[cfg(not(windows))]
mod imp {
    pub fn promote() -> bool {
        false
    }
}
