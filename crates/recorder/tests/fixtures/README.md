# Fichiers de test du décodeur

Tous : 0,5 s, 44 100 Hz, stéréo, amplitude 0,0625 (sine de ffmpeg = 0,125, puis volume 0,5) — 1 kHz à gauche, 2 kHz à droite. Générés avec ffmpeg :

```
F='-f lavfi -i sine=frequency=1000:sample_rate=44100:duration=0.5 -f lavfi -i sine=frequency=2000:sample_rate=44100:duration=0.5 -filter_complex [0][1]amerge=inputs=2,volume=0.5 -y'
ffmpeg $F -c:a flac -sample_fmt s16 st16.flac
ffmpeg $F -c:a flac -sample_fmt s32 -bits_per_raw_sample 24 st24.flac
ffmpeg $F -c:a libmp3lame -b:a 192k st.mp3
ffmpeg $F -c:a pcm_s24le st24.wav ; ffmpeg $F -c:a pcm_s16le st16.wav ; ffmpeg $F -c:a pcm_u8 st8.wav
```
