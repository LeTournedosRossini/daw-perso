//! File des échantillons capturés, partagée par les backends ALSA (Linux) et WASAPI (Windows).

/// File des échantillons capturés, par canal (taille fixe, sans allocation après la création). La capture et la lecture ont chacune
/// leur horloge : la lecture (bloquante) cadence le cycle, la capture est lue sans attendre et passe par cette file, qui absorbe la
/// dérive entre les deux horloges.
pub struct CaptureRing {
    data: Vec<Vec<f32>>,
    start: usize,
    len: usize,
}

impl CaptureRing {
    pub fn new(channels: usize, capacity: usize) -> Self {
        CaptureRing { data: (0..channels).map(|_| vec![0.0; capacity]).collect(), start: 0, len: 0 }
    }
    fn capacity(&self) -> usize {
        self.data.first().map_or(0, |d| d.len())
    }
    /// Ajoute `frames` trames entrelacées (`channels` valeurs par trame) ; ce qui dépasse écrase le plus ancien.
    pub fn push(&mut self, interleaved: &[f32], channels: usize, frames: usize) {
        let cap = self.capacity();
        if cap == 0 {
            return;
        }
        for f in 0..frames {
            let at = (self.start + self.len) % cap;
            for c in 0..channels.min(self.data.len()) {
                self.data[c][at] = interleaved[f * channels + c];
            }
            if self.len < cap {
                self.len += 1;
            } else {
                self.start = (self.start + 1) % cap;
            }
        }
    }
    /// Retire `out[c].len()` trames si elles sont toutes là ; rend faux (sans rien retirer) sinon.
    pub fn pop(&mut self, out: &mut [Vec<f32>]) -> bool {
        let n = out.first().map_or(0, |o| o.len());
        self.pop_n(out, n)
    }

    /// Comme `pop`, pour les `n` premières trames de chaque canal de `out` (les tampons peuvent être plus longs).
    pub fn pop_n(&mut self, out: &mut [Vec<f32>], n: usize) -> bool {
        if self.len < n {
            return false;
        }
        let cap = self.capacity();
        for (c, o) in out.iter_mut().enumerate() {
            for (i, v) in o.iter_mut().take(n).enumerate() {
                *v = self.data.get(c).map_or(0.0, |d| d[(self.start + i) % cap]);
            }
        }
        self.start = (self.start + n) % cap;
        self.len -= n;
        true
    }
    /// Garde au plus `keep` trames (les plus récentes) : rattrape une capture qui prend de l'avance sur la lecture.
    pub fn trim(&mut self, keep: usize) {
        if self.len > keep {
            let cap = self.capacity();
            self.start = (self.start + self.len - keep) % cap;
            self.len = keep;
        }
    }
}

impl CaptureRing {
    /// Nombre de trames en attente.
    pub fn len(&self) -> usize {
        self.len
    }

    pub fn is_empty(&self) -> bool {
        self.len == 0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn planar(channels: usize, n: usize) -> Vec<Vec<f32>> {
        (0..channels).map(|_| vec![0.0; n]).collect()
    }

    #[test]
    fn the_capture_ring_hands_back_what_was_pushed_in_order_per_channel() {
        let mut r = CaptureRing::new(2, 16);
        // Trois trames entrelacées : (1,10) (2,20) (3,30).
        r.push(&[1.0, 10.0, 2.0, 20.0, 3.0, 30.0], 2, 3);
        let mut out = planar(2, 2);
        assert!(r.pop(&mut out));
        assert_eq!(out[0], [1.0, 2.0]);
        assert_eq!(out[1], [10.0, 20.0]);
        // Il en reste une : pas assez pour deux trames, rien n'est retiré.
        let mut two = planar(2, 2);
        assert!(!r.pop(&mut two));
        let mut one = planar(2, 1);
        assert!(r.pop(&mut one));
        assert_eq!((one[0][0], one[1][0]), (3.0, 30.0));
    }

    #[test]
    fn the_capture_ring_overwrites_the_oldest_when_full_and_trims_to_the_newest() {
        let mut r = CaptureRing::new(1, 4);
        r.push(&[1.0, 2.0, 3.0, 4.0, 5.0, 6.0], 1, 6);
        assert_eq!(r.len(), 4);
        let mut out = planar(1, 4);
        assert!(r.pop(&mut out));
        assert_eq!(out[0], [3.0, 4.0, 5.0, 6.0]);
        r.push(&[7.0, 8.0, 9.0], 1, 3);
        r.trim(1);
        let mut last = planar(1, 1);
        assert!(r.pop(&mut last));
        assert_eq!(last[0][0], 9.0, "on garde les trames les plus récentes");
    }
}
