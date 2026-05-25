# Parser Documentation

## Parser Inventory

Progress is measured against the corresponding parser in `../mkvtoolnix` for BatchMkvMerge's native parser scope: format recognition and header-level metadata extraction into `MediaMetadata`. Muxing-only work from mkvmerge, such as packetizer creation, packet delivery, timestamp rewriting, and output-file authoring, is documented as an intentional gap but is not counted against the percentage.

| Parser | Description | Implementation | Progress |
| --- | --- | --- | --- |
| [Matroska / WebM](matroska/matroska.md) | EBML-based Matroska/WebM reader for segment info, tracks, chapters, attachments, tags, cues, and cluster timestamp hints. | `src-tauri/src/media_metadata/matroska/reader.rs` | 86% |
| [AVI](avi/avi.md) | RIFF/AVI reader for header lists, ODML metadata, video/audio streams, and embedded subtitle hints. | `src-tauri/src/media_metadata/avi/reader.rs` | 76% |
| [Ogg / OGM](ogg/ogg.md) | Ogg page reader with codec sniffers for Vorbis, Opus, Theora, FLAC, Speex, Kate, and legacy OGM. | `src-tauri/src/media_metadata/ogg/reader.rs` | 80% |
| [MP4 / QuickTime](mp4/mp4.md) | ISO BMFF/QuickTime reader for movie boxes, sample entries, codec-specific boxes, metadata, fragments, and first-sample verification. | `src-tauri/src/media_metadata/mp4/reader.rs` | 85% |
| [MPEG Program Stream](mpeg_ps/mpeg_ps.md) | MPEG-PS start-code walker with program-stream-map support and PES payload enrichment. | `src-tauri/src/media_metadata/mpeg_ps/reader.rs` | 72% |
| [MPEG Transport Stream](mpeg_ts/mpeg_ts.md) | MPEG-TS packet reader for PAT, PMT, SDT, descriptors, PID tables, and PES-based stream enrichment. | `src-tauri/src/media_metadata/mpeg_ts/reader.rs` | 84% |
| [FLV](flv/flv.md) | Flash Video reader for file headers, tags, script metadata, AAC/H.264/H.265 configs, and legacy FLV codecs. | `src-tauri/src/media_metadata/flv/reader.rs` | 82% |
| [RealMedia](realmedia/realmedia.md) | RealMedia chunk reader for PROP, CONT, MDPR, DATA, RealVideo, RealAudio, and first-packet refinements. | `src-tauri/src/media_metadata/realmedia/reader.rs` | 76% |
| [IVF](ivf.md) | IVF header reader for AV1, VP8, VP9, frame-rate defaults, and AV1 Dolby Vision RPU hints. | `src-tauri/src/media_metadata/ivf.rs` | 95% |
| [Blu-ray MPLS Playlist](mpls/mpls.md) | Blu-ray playlist parser that resolves clip chains, playlist metadata, chapters, and stream languages before delegating to MPEG-TS parsing. | `src-tauri/src/media_metadata/mpls/mod.rs` | 82% |
| [AAC](audio/aac.md) | ADTS and LOAS/LATM AAC reader with AudioSpecificConfig parsing and ID3 skipping. | `src-tauri/src/media_metadata/audio/aac.rs` | 90% |
| [AC-3 / E-AC-3](audio/ac3.md) | AC-3 and E-AC-3 frame-sync reader for sample rate, channels, bitrate, and variant detection. | `src-tauri/src/media_metadata/audio/ac3.rs` | 84% |
| [DTS / DTS-HD](audio/dts.md) | DTS core and DTS-HD reader with endian/14-bit transforms, channel masks, bit depth, and extension detection. | `src-tauri/src/media_metadata/audio/dts.rs` | 94% |
| [FLAC](audio/flac.md) | Native FLAC reader for STREAMINFO, VorbisComment, picture attachments, and ID3-prefixed files. | `src-tauri/src/media_metadata/audio/flac.rs` | 94% |
| [MP3 / MPEG Audio](audio/mp3.md) | MPEG audio frame reader for Layers I, II, III with ID3v2 and ID3v1 trimming. | `src-tauri/src/media_metadata/audio/mp3.rs` | 97% |
| [TrueHD / MLP](audio/truehd.md) | Dolby TrueHD/MLP reader with major-sync parsing and coupled AC-3 substream reporting. | `src-tauri/src/media_metadata/audio/truehd.rs` | 92% |
| [TTA](audio/tta.md) | TTA1 reader for stream header, seek-table validation, duration, and audio properties. | `src-tauri/src/media_metadata/audio/tta.rs` | 78% |
| [WAV / RF64 / Wave64](audio/wav.md) | WAV-family reader for RIFF, RF64, Wave64, WAVEFORMATEX/TENSIBLE, PCM, AC-3, and DTS payloads. | `src-tauri/src/media_metadata/audio/wav.rs` | 88% |
| [WavPack](audio/wavpack.md) | WavPack v4 frame reader for sample rate, channels, bit depth, DSD rate hints, and duration. | `src-tauri/src/media_metadata/audio/wavpack.rs` | 90% |
| [CoreAudio CAF](coreaudio/coreaudio.md) | CAF reader for desc/data/pakt/kuki chunks, ALAC cookies, and audio properties. | `src-tauri/src/media_metadata/coreaudio/reader.rs` | 90% |
| [AVC / H.264 Elementary Stream](elementary/avc/avc.md) | Annex B H.264 reader for SPS/PPS discovery, codec-private generation, dimensions, profile, level, and VUI timing. | `src-tauri/src/media_metadata/elementary/avc/reader.rs` | 78% |
| [HEVC / H.265 Elementary Stream](elementary/hevc/hevc.md) | Annex B H.265 reader for VPS/SPS/PPS discovery, codec-private generation, profile-tier-level, dimensions, and VUI timing. | `src-tauri/src/media_metadata/elementary/hevc/reader.rs` | 72% |
| [MPEG-1/2 Video Elementary Stream](elementary/mpeg_video.md) | MPEG video elementary-stream reader for sequence headers, sequence extensions, dimensions, and frame-rate defaults. | `src-tauri/src/media_metadata/elementary/mpeg_video.rs` | 82% |
| [VC-1 Elementary Stream](elementary/vc1.md) | VC-1 advanced-profile elementary-stream reader for sequence headers and dimensions. | `src-tauri/src/media_metadata/elementary/vc1.rs` | 58% |
| [Dirac Elementary Stream](elementary/dirac.md) | Dirac parse-info reader for sequence headers, standard video formats, dimensions, and frame duration. | `src-tauri/src/media_metadata/elementary/dirac.rs` | 62% |
| [DV](elementary/dv.md) | DV signature prober that mirrors mkvtoolnix's unsupported-format handling for raw DV streams. | `src-tauri/src/media_metadata/elementary/dv.rs` | 45% |
| [AV1 OBU](elementary/obu.md) | AV1 Open Bitstream Units reader for sequence headers, frame presence, profile, bit depth, color, and dimensions. | `src-tauri/src/media_metadata/elementary/obu.rs` | 68% |
| [SRT](subtitles/srt.md) | SubRip text subtitle reader with encoding detection, timecode probing, and empty-file extension fallback. | `src-tauri/src/media_metadata/subtitles/srt.rs` | 82% |
| [SSA / ASS](subtitles/ssa.md) | SSA/ASS text subtitle reader for variant detection, global headers, language/name metadata, and embedded font attachments. | `src-tauri/src/media_metadata/subtitles/ssa.rs` | 72% |
| [WebVTT](subtitles/webvtt.md) | WebVTT reader for strict WEBVTT signatures, header preservation, and encoding detection. | `src-tauri/src/media_metadata/subtitles/webvtt.rs` | 80% |
| [USF](subtitles/usf.md) | USF XML subtitle reader for root detection, multiple subtitle elements, language/name extraction, and text tracks. | `src-tauri/src/media_metadata/subtitles/usf.rs` | 45% |
| [MicroDVD](subtitles/microdvd.md) | MicroDVD signature prober that mirrors mkvtoolnix's unsupported-format behavior. | `src-tauri/src/media_metadata/subtitles/microdvd.rs` | 88% |
| [VobSub IDX](subtitles/vobsub.md) | VobSub `.idx` reader for language entries, sibling `.sub` discovery, and one subtitle track per entry. | `src-tauri/src/media_metadata/subtitles/vobsub.rs` | 58% |
| [PGS SUP](subtitles/pgs.md) | HDMV PGS `.sup` reader for segment-chain validation and image subtitle track metadata. | `src-tauri/src/media_metadata/subtitles/pgs.rs` | 90% |
| [HDMV TextST](subtitles/hdmv_textst.md) | HDMV TextST reader for segment validation, Dialog Style codec-private data, and text subtitle track metadata. | `src-tauri/src/media_metadata/subtitles/hdmv_textst.rs` | 85% |
| [VobButton](subtitles/vobbtn.md) | VobButton reader for button-stream magic, PES structure validation, and button track metadata. | `src-tauri/src/media_metadata/subtitles/vobbtn.rs` | 98% |
