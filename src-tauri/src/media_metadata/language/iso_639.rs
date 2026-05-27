/*
 *   Copyright (c) 2026. caoccao.com Sam Cao
 *   All rights reserved.

 *   Licensed under the Apache License, Version 2.0 (the "License");
 *   you may not use this file except in compliance with the License.
 *   You may obtain a copy of the License at

 *   http://www.apache.org/licenses/LICENSE-2.0

 *   Unless required by applicable law or agreed to in writing, software
 *   distributed under the License is distributed on an "AS IS" BASIS,
 *   WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
 *   See the License for the specific language governing permissions and
 *   limitations under the License.
 */

//! Static ISO 639-2 alpha-3 code table.
//!
//! Sourced from the Library of Congress registry
//! (<https://www.loc.gov/standards/iso639-2/php/code_list.php>).
//!
//! Both bibliographic (`B`) and terminologic (`T`) codes are recognised for
//! lookup, with the terminologic code preferred as the canonical form when
//! both exist (e.g. `fre` → `fra`, `ger` → `deu`, `chi` → `zho`).  This
//! mirrors how IETF BCP-47 and modern container muxers treat the pair.

/// Result of looking up a 3-letter code.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Iso639Match<'a> {
  /// Canonical (terminologic) alpha-3 code.
  pub canonical: &'a str,
  /// English name for display.
  pub name: &'a str,
}

/// Look up a 3-letter code.  Returns `Some` for any code in the official
/// registry, including bibliographic-only entries.  Lookup is
/// case-insensitive and ignores surrounding whitespace.
pub fn lookup(code: &str) -> Option<Iso639Match<'static>> {
  let normalized = code.trim().to_ascii_lowercase();
  if normalized.len() != 3 {
    return None;
  }
  // Bibliographic alias → terminologic translation first.
  if let Some(canonical) = bib_to_term(&normalized) {
    // The terminologic entry is the canonical one — look it up by code.
    return TABLE
      .binary_search_by_key(&canonical, |entry| entry.code)
      .ok()
      .map(|i| TABLE[i])
      .map(|e| Iso639Match {
        canonical: e.code,
        name: e.name,
      });
  }
  TABLE
    .binary_search_by_key(&normalized.as_str(), |entry| entry.code)
    .ok()
    .map(|i| Iso639Match {
      canonical: TABLE[i].code,
      name: TABLE[i].name,
    })
}

/// `true` if `code` is a recognised ISO 639-2 alpha-3 code.
pub fn is_valid(code: &str) -> bool {
  lookup(code).is_some()
}

/// Reverse of [`bib_to_term`]: a terminologic code → its bibliographic twin,
/// when the pair exists (`fra` → `fre`, `deu` → `ger`, `zho` → `chi`).
/// `None` when the language has no distinct bibliographic code.
pub fn term_to_bib(term: &str) -> Option<&'static str> {
  let t = term.trim().to_ascii_lowercase();
  BIB_T_PAIRS.iter().copied().find_map(|(b, tt)| if tt == t { Some(b) } else { None })
}

/// Best-effort ISO 639-2 (canonical or bibliographic) → ISO 639-1 alpha-2.
/// Returns the alpha-2 whose registry row resolves to the same canonical
/// language (`fra`/`fre` → `fr`, `deu`/`ger` → `de`).  `None` when the
/// language has no alpha-2 code.
pub fn alpha3_to_alpha2(code: &str) -> Option<&'static str> {
  let canonical = lookup(code)?.canonical;
  ALPHA2_TO_ALPHA3
    .iter()
    .copied()
    .find(|(_, a3)| lookup(a3).map(|m| m.canonical) == Some(canonical))
    .map(|(a2, _)| a2)
}

/// Map an ISO 639-1 alpha-2 code (`en`, `ja`, `pt`, ...) to its ISO 639-2
/// alpha-3 code.  The returned code may be a bibliographic alias (e.g.
/// `de → ger`); feed it through [`lookup`] to obtain the canonical
/// terminologic form.  Case-insensitive, whitespace-tolerant.
///
/// Mirrors the `alpha_2_code → alpha_3_code` column of mkvtoolnix's
/// `common/iso639_language_list.cpp`.
pub fn alpha2_to_alpha3(code: &str) -> Option<&'static str> {
  let normalized = code.trim().to_ascii_lowercase();
  if normalized.len() != 2 {
    return None;
  }
  ALPHA2_TO_ALPHA3
    .binary_search_by_key(&normalized.as_str(), |(a2, _)| a2)
    .ok()
    .map(|i| ALPHA2_TO_ALPHA3[i].1)
}

/// The fixed "no linguistic content" code from the registry.
pub const ZXX: &str = "zxx";
/// The fixed "undetermined" code from the registry.
pub const UND: &str = "und";

/// Translate a bibliographic ISO 639-2/B code to its terminologic (T) twin.
/// Returns `None` if the input is not a bibliographic alias.
fn bib_to_term(code: &str) -> Option<&'static str> {
  // Sorted by bibliographic code so we can do a linear scan or
  // binary search.  There are only ~22 pairs, so linear is fine.
  BIB_T_PAIRS
    .iter()
    .copied()
    .find_map(|(b, t)| if b == code { Some(t) } else { None })
}

/// ISO 639-1 alpha-2 → ISO 639-2 alpha-3 mapping, sorted by alpha-2 for
/// binary search.  Some targets are bibliographic codes (`de → ger`,
/// `fr → fre`, `zh → chi`); [`lookup`] resolves those to the terminologic
/// canonical form.
#[rustfmt::skip]
const ALPHA2_TO_ALPHA3: &[(&str, &str)] = &[
    ("aa", "aar"), ("ab", "abk"), ("ae", "ave"), ("af", "afr"),
    ("ak", "aka"), ("am", "amh"), ("an", "arg"), ("ar", "ara"),
    ("as", "asm"), ("av", "ava"), ("ay", "aym"), ("az", "aze"),
    ("ba", "bak"), ("be", "bel"), ("bg", "bul"), ("bi", "bis"),
    ("bm", "bam"), ("bn", "ben"), ("bo", "tib"), ("br", "bre"),
    ("bs", "bos"), ("ca", "cat"), ("ce", "che"), ("ch", "cha"),
    ("co", "cos"), ("cr", "cre"), ("cs", "cze"), ("cu", "chu"),
    ("cv", "chv"), ("cy", "wel"), ("da", "dan"), ("de", "ger"),
    ("dv", "div"), ("dz", "dzo"), ("ee", "ewe"), ("el", "gre"),
    ("en", "eng"), ("eo", "epo"), ("es", "spa"), ("et", "est"),
    ("eu", "baq"), ("fa", "per"), ("ff", "ful"), ("fi", "fin"),
    ("fj", "fij"), ("fo", "fao"), ("fr", "fre"), ("fy", "fry"),
    ("ga", "gle"), ("gd", "gla"), ("gl", "glg"), ("gn", "grn"),
    ("gu", "guj"), ("gv", "glv"), ("ha", "hau"), ("he", "heb"),
    ("hi", "hin"), ("ho", "hmo"), ("hr", "hrv"), ("ht", "hat"),
    ("hu", "hun"), ("hy", "arm"), ("hz", "her"), ("ia", "ina"),
    ("id", "ind"), ("ie", "ile"), ("ig", "ibo"), ("ii", "iii"),
    ("ik", "ipk"), ("io", "ido"), ("is", "ice"), ("it", "ita"),
    ("iu", "iku"), ("ja", "jpn"), ("jv", "jav"), ("ka", "geo"),
    ("kg", "kon"), ("ki", "kik"), ("kj", "kua"), ("kk", "kaz"),
    ("kl", "kal"), ("km", "khm"), ("kn", "kan"), ("ko", "kor"),
    ("kr", "kau"), ("ks", "kas"), ("ku", "kur"), ("kv", "kom"),
    ("kw", "cor"), ("ky", "kir"), ("la", "lat"), ("lb", "ltz"),
    ("lg", "lug"), ("li", "lim"), ("ln", "lin"), ("lo", "lao"),
    ("lt", "lit"), ("lu", "lub"), ("lv", "lav"), ("mg", "mlg"),
    ("mh", "mah"), ("mi", "mao"), ("mk", "mac"), ("ml", "mal"),
    ("mn", "mon"), ("mr", "mar"), ("ms", "may"), ("mt", "mlt"),
    ("my", "bur"), ("na", "nau"), ("nb", "nob"), ("nd", "nde"),
    ("ne", "nep"), ("ng", "ndo"), ("nl", "dut"), ("nn", "nno"),
    ("no", "nor"), ("nr", "nbl"), ("nv", "nav"), ("ny", "nya"),
    ("oc", "oci"), ("oj", "oji"), ("om", "orm"), ("or", "ori"),
    ("os", "oss"), ("pa", "pan"), ("pi", "pli"), ("pl", "pol"),
    ("ps", "pus"), ("pt", "por"), ("qu", "que"), ("rm", "roh"),
    ("rn", "run"), ("ro", "rum"), ("ru", "rus"), ("rw", "kin"),
    ("sa", "san"), ("sc", "srd"), ("sd", "snd"), ("se", "sme"),
    ("sg", "sag"), ("sh", "hbs"), ("si", "sin"), ("sk", "slo"),
    ("sl", "slv"), ("sm", "smo"), ("sn", "sna"), ("so", "som"),
    ("sq", "alb"), ("sr", "srp"), ("ss", "ssw"), ("st", "sot"),
    ("su", "sun"), ("sv", "swe"), ("sw", "swa"), ("ta", "tam"),
    ("te", "tel"), ("tg", "tgk"), ("th", "tha"), ("ti", "tir"),
    ("tk", "tuk"), ("tl", "tgl"), ("tn", "tsn"), ("to", "ton"),
    ("tr", "tur"), ("ts", "tso"), ("tt", "tat"), ("tw", "twi"),
    ("ty", "tah"), ("ug", "uig"), ("uk", "ukr"), ("ur", "urd"),
    ("uz", "uzb"), ("ve", "ven"), ("vi", "vie"), ("vo", "vol"),
    ("wa", "wln"), ("wo", "wol"), ("xh", "xho"), ("yi", "yid"),
    ("yo", "yor"), ("za", "zha"), ("zh", "chi"), ("zu", "zul"),
];

const BIB_T_PAIRS: &[(&str, &str)] = &[
  ("alb", "sqi"),
  ("arm", "hye"),
  ("baq", "eus"),
  ("bur", "mya"),
  ("chi", "zho"),
  ("cze", "ces"),
  ("dut", "nld"),
  ("fre", "fra"),
  ("geo", "kat"),
  ("ger", "deu"),
  ("gre", "ell"),
  ("ice", "isl"),
  ("mac", "mkd"),
  ("mao", "mri"),
  ("may", "msa"),
  ("per", "fas"),
  ("rum", "ron"),
  ("slo", "slk"),
  ("tib", "bod"),
  ("wel", "cym"),
  ("scc", "srp"),
  ("scr", "hrv"),
];

#[derive(Debug, Clone, Copy)]
struct Entry {
  code: &'static str,
  name: &'static str,
}

/// The full ISO 639-2 alpha-3 table.
///
/// Sorted by `code` so [`lookup`] can use binary search.  Bibliographic-only
/// aliases (e.g. `fre`) are intentionally NOT included here — they are
/// resolved to their terminologic twin by [`bib_to_term`] before this slice
/// is consulted.  This keeps the canonical-name mapping single-valued.
#[rustfmt::skip]
const TABLE: &[Entry] = &[
    Entry { code: "aar", name: "Afar" },
    Entry { code: "abk", name: "Abkhazian" },
    Entry { code: "ace", name: "Achinese" },
    Entry { code: "ach", name: "Acoli" },
    Entry { code: "ada", name: "Adangme" },
    Entry { code: "ady", name: "Adyghe" },
    Entry { code: "afa", name: "Afro-Asiatic languages" },
    Entry { code: "afh", name: "Afrihili" },
    Entry { code: "afr", name: "Afrikaans" },
    Entry { code: "ain", name: "Ainu" },
    Entry { code: "aka", name: "Akan" },
    Entry { code: "akk", name: "Akkadian" },
    Entry { code: "ale", name: "Aleut" },
    Entry { code: "alg", name: "Algonquian languages" },
    Entry { code: "alt", name: "Southern Altai" },
    Entry { code: "amh", name: "Amharic" },
    Entry { code: "ang", name: "English, Old (ca.450-1100)" },
    Entry { code: "anp", name: "Angika" },
    Entry { code: "apa", name: "Apache languages" },
    Entry { code: "ara", name: "Arabic" },
    Entry { code: "arc", name: "Official Aramaic (700-300 BCE)" },
    Entry { code: "arg", name: "Aragonese" },
    Entry { code: "arn", name: "Mapudungun" },
    Entry { code: "arp", name: "Arapaho" },
    Entry { code: "art", name: "Artificial languages" },
    Entry { code: "arw", name: "Arawak" },
    Entry { code: "asm", name: "Assamese" },
    Entry { code: "ast", name: "Asturian" },
    Entry { code: "ath", name: "Athapascan languages" },
    Entry { code: "aus", name: "Australian languages" },
    Entry { code: "ava", name: "Avaric" },
    Entry { code: "ave", name: "Avestan" },
    Entry { code: "awa", name: "Awadhi" },
    Entry { code: "aym", name: "Aymara" },
    Entry { code: "aze", name: "Azerbaijani" },
    Entry { code: "bad", name: "Banda languages" },
    Entry { code: "bai", name: "Bamileke languages" },
    Entry { code: "bak", name: "Bashkir" },
    Entry { code: "bal", name: "Baluchi" },
    Entry { code: "bam", name: "Bambara" },
    Entry { code: "ban", name: "Balinese" },
    Entry { code: "bas", name: "Basa" },
    Entry { code: "bat", name: "Baltic languages" },
    Entry { code: "bej", name: "Beja" },
    Entry { code: "bel", name: "Belarusian" },
    Entry { code: "bem", name: "Bemba" },
    Entry { code: "ben", name: "Bengali" },
    Entry { code: "ber", name: "Berber languages" },
    Entry { code: "bho", name: "Bhojpuri" },
    Entry { code: "bih", name: "Bihari languages" },
    Entry { code: "bik", name: "Bikol" },
    Entry { code: "bin", name: "Bini" },
    Entry { code: "bis", name: "Bislama" },
    Entry { code: "bla", name: "Siksika" },
    Entry { code: "bnt", name: "Bantu languages" },
    Entry { code: "bod", name: "Tibetan" },
    Entry { code: "bos", name: "Bosnian" },
    Entry { code: "bra", name: "Braj" },
    Entry { code: "bre", name: "Breton" },
    Entry { code: "btk", name: "Batak languages" },
    Entry { code: "bua", name: "Buriat" },
    Entry { code: "bug", name: "Buginese" },
    Entry { code: "bul", name: "Bulgarian" },
    Entry { code: "byn", name: "Blin" },
    Entry { code: "cad", name: "Caddo" },
    Entry { code: "cai", name: "Central American Indian languages" },
    Entry { code: "car", name: "Galibi Carib" },
    Entry { code: "cat", name: "Catalan" },
    Entry { code: "cau", name: "Caucasian languages" },
    Entry { code: "ceb", name: "Cebuano" },
    Entry { code: "cel", name: "Celtic languages" },
    Entry { code: "ces", name: "Czech" },
    Entry { code: "cha", name: "Chamorro" },
    Entry { code: "chb", name: "Chibcha" },
    Entry { code: "che", name: "Chechen" },
    Entry { code: "chg", name: "Chagatai" },
    Entry { code: "chk", name: "Chuukese" },
    Entry { code: "chm", name: "Mari" },
    Entry { code: "chn", name: "Chinook jargon" },
    Entry { code: "cho", name: "Choctaw" },
    Entry { code: "chp", name: "Chipewyan" },
    Entry { code: "chr", name: "Cherokee" },
    Entry { code: "chu", name: "Church Slavic" },
    Entry { code: "chv", name: "Chuvash" },
    Entry { code: "chy", name: "Cheyenne" },
    Entry { code: "cmc", name: "Chamic languages" },
    Entry { code: "cnr", name: "Montenegrin" },
    Entry { code: "cop", name: "Coptic" },
    Entry { code: "cor", name: "Cornish" },
    Entry { code: "cos", name: "Corsican" },
    Entry { code: "cpe", name: "Creoles and pidgins, English based" },
    Entry { code: "cpf", name: "Creoles and pidgins, French-based" },
    Entry { code: "cpp", name: "Creoles and pidgins, Portuguese-based" },
    Entry { code: "cre", name: "Cree" },
    Entry { code: "crh", name: "Crimean Tatar" },
    Entry { code: "crp", name: "Creoles and pidgins" },
    Entry { code: "csb", name: "Kashubian" },
    Entry { code: "cus", name: "Cushitic languages" },
    Entry { code: "cym", name: "Welsh" },
    Entry { code: "dak", name: "Dakota" },
    Entry { code: "dan", name: "Danish" },
    Entry { code: "dar", name: "Dargwa" },
    Entry { code: "day", name: "Land Dayak languages" },
    Entry { code: "del", name: "Delaware" },
    Entry { code: "den", name: "Slave (Athapascan)" },
    Entry { code: "deu", name: "German" },
    Entry { code: "dgr", name: "Dogrib" },
    Entry { code: "din", name: "Dinka" },
    Entry { code: "div", name: "Divehi" },
    Entry { code: "doi", name: "Dogri" },
    Entry { code: "dra", name: "Dravidian languages" },
    Entry { code: "dsb", name: "Lower Sorbian" },
    Entry { code: "dua", name: "Duala" },
    Entry { code: "dum", name: "Dutch, Middle (ca.1050-1350)" },
    Entry { code: "dyu", name: "Dyula" },
    Entry { code: "dzo", name: "Dzongkha" },
    Entry { code: "efi", name: "Efik" },
    Entry { code: "egy", name: "Egyptian (Ancient)" },
    Entry { code: "eka", name: "Ekajuk" },
    Entry { code: "ell", name: "Greek, Modern (1453-)" },
    Entry { code: "elx", name: "Elamite" },
    Entry { code: "eng", name: "English" },
    Entry { code: "enm", name: "English, Middle (1100-1500)" },
    Entry { code: "epo", name: "Esperanto" },
    Entry { code: "est", name: "Estonian" },
    Entry { code: "eus", name: "Basque" },
    Entry { code: "ewe", name: "Ewe" },
    Entry { code: "ewo", name: "Ewondo" },
    Entry { code: "fan", name: "Fang" },
    Entry { code: "fao", name: "Faroese" },
    Entry { code: "fas", name: "Persian" },
    Entry { code: "fat", name: "Fanti" },
    Entry { code: "fij", name: "Fijian" },
    Entry { code: "fil", name: "Filipino" },
    Entry { code: "fin", name: "Finnish" },
    Entry { code: "fiu", name: "Finno-Ugrian languages" },
    Entry { code: "fon", name: "Fon" },
    Entry { code: "fra", name: "French" },
    Entry { code: "frm", name: "French, Middle (ca.1400-1600)" },
    Entry { code: "fro", name: "French, Old (842-ca.1400)" },
    Entry { code: "frr", name: "Northern Frisian" },
    Entry { code: "frs", name: "Eastern Frisian" },
    Entry { code: "fry", name: "Western Frisian" },
    Entry { code: "ful", name: "Fulah" },
    Entry { code: "fur", name: "Friulian" },
    Entry { code: "gaa", name: "Ga" },
    Entry { code: "gay", name: "Gayo" },
    Entry { code: "gba", name: "Gbaya" },
    Entry { code: "gem", name: "Germanic languages" },
    Entry { code: "gez", name: "Geez" },
    Entry { code: "gil", name: "Gilbertese" },
    Entry { code: "gla", name: "Gaelic" },
    Entry { code: "gle", name: "Irish" },
    Entry { code: "glg", name: "Galician" },
    Entry { code: "glv", name: "Manx" },
    Entry { code: "gmh", name: "German, Middle High (ca.1050-1500)" },
    Entry { code: "goh", name: "German, Old High (ca.750-1050)" },
    Entry { code: "gon", name: "Gondi" },
    Entry { code: "gor", name: "Gorontalo" },
    Entry { code: "got", name: "Gothic" },
    Entry { code: "grb", name: "Grebo" },
    Entry { code: "grc", name: "Greek, Ancient (to 1453)" },
    Entry { code: "grn", name: "Guarani" },
    Entry { code: "gsw", name: "Swiss German" },
    Entry { code: "guj", name: "Gujarati" },
    Entry { code: "gwi", name: "Gwich'in" },
    Entry { code: "hai", name: "Haida" },
    Entry { code: "hat", name: "Haitian" },
    Entry { code: "hau", name: "Hausa" },
    Entry { code: "haw", name: "Hawaiian" },
    Entry { code: "heb", name: "Hebrew" },
    Entry { code: "her", name: "Herero" },
    Entry { code: "hil", name: "Hiligaynon" },
    Entry { code: "him", name: "Himachali languages" },
    Entry { code: "hin", name: "Hindi" },
    Entry { code: "hit", name: "Hittite" },
    Entry { code: "hmn", name: "Hmong" },
    Entry { code: "hmo", name: "Hiri Motu" },
    Entry { code: "hrv", name: "Croatian" },
    Entry { code: "hsb", name: "Upper Sorbian" },
    Entry { code: "hun", name: "Hungarian" },
    Entry { code: "hup", name: "Hupa" },
    Entry { code: "hye", name: "Armenian" },
    Entry { code: "iba", name: "Iban" },
    Entry { code: "ibo", name: "Igbo" },
    Entry { code: "ido", name: "Ido" },
    Entry { code: "iii", name: "Sichuan Yi" },
    Entry { code: "ijo", name: "Ijo languages" },
    Entry { code: "iku", name: "Inuktitut" },
    Entry { code: "ile", name: "Interlingue" },
    Entry { code: "ilo", name: "Iloko" },
    Entry { code: "ina", name: "Interlingua (International Auxiliary Language Association)" },
    Entry { code: "inc", name: "Indic languages" },
    Entry { code: "ind", name: "Indonesian" },
    Entry { code: "ine", name: "Indo-European languages" },
    Entry { code: "inh", name: "Ingush" },
    Entry { code: "ipk", name: "Inupiaq" },
    Entry { code: "ira", name: "Iranian languages" },
    Entry { code: "iro", name: "Iroquoian languages" },
    Entry { code: "isl", name: "Icelandic" },
    Entry { code: "ita", name: "Italian" },
    Entry { code: "jav", name: "Javanese" },
    Entry { code: "jbo", name: "Lojban" },
    Entry { code: "jpn", name: "Japanese" },
    Entry { code: "jpr", name: "Judeo-Persian" },
    Entry { code: "jrb", name: "Judeo-Arabic" },
    Entry { code: "kaa", name: "Kara-Kalpak" },
    Entry { code: "kab", name: "Kabyle" },
    Entry { code: "kac", name: "Kachin" },
    Entry { code: "kal", name: "Kalaallisut" },
    Entry { code: "kam", name: "Kamba" },
    Entry { code: "kan", name: "Kannada" },
    Entry { code: "kar", name: "Karen languages" },
    Entry { code: "kas", name: "Kashmiri" },
    Entry { code: "kat", name: "Georgian" },
    Entry { code: "kau", name: "Kanuri" },
    Entry { code: "kaw", name: "Kawi" },
    Entry { code: "kaz", name: "Kazakh" },
    Entry { code: "kbd", name: "Kabardian" },
    Entry { code: "kha", name: "Khasi" },
    Entry { code: "khi", name: "Khoisan languages" },
    Entry { code: "khm", name: "Central Khmer" },
    Entry { code: "kho", name: "Khotanese" },
    Entry { code: "kik", name: "Kikuyu" },
    Entry { code: "kin", name: "Kinyarwanda" },
    Entry { code: "kir", name: "Kirghiz" },
    Entry { code: "kmb", name: "Kimbundu" },
    Entry { code: "kok", name: "Konkani" },
    Entry { code: "kom", name: "Komi" },
    Entry { code: "kon", name: "Kongo" },
    Entry { code: "kor", name: "Korean" },
    Entry { code: "kos", name: "Kosraean" },
    Entry { code: "kpe", name: "Kpelle" },
    Entry { code: "krc", name: "Karachay-Balkar" },
    Entry { code: "krl", name: "Karelian" },
    Entry { code: "kro", name: "Kru languages" },
    Entry { code: "kru", name: "Kurukh" },
    Entry { code: "kua", name: "Kuanyama" },
    Entry { code: "kum", name: "Kumyk" },
    Entry { code: "kur", name: "Kurdish" },
    Entry { code: "kut", name: "Kutenai" },
    Entry { code: "lad", name: "Ladino" },
    Entry { code: "lah", name: "Lahnda" },
    Entry { code: "lam", name: "Lamba" },
    Entry { code: "lao", name: "Lao" },
    Entry { code: "lat", name: "Latin" },
    Entry { code: "lav", name: "Latvian" },
    Entry { code: "lez", name: "Lezghian" },
    Entry { code: "lim", name: "Limburgan" },
    Entry { code: "lin", name: "Lingala" },
    Entry { code: "lit", name: "Lithuanian" },
    Entry { code: "lol", name: "Mongo" },
    Entry { code: "loz", name: "Lozi" },
    Entry { code: "ltz", name: "Luxembourgish" },
    Entry { code: "lua", name: "Luba-Lulua" },
    Entry { code: "lub", name: "Luba-Katanga" },
    Entry { code: "lug", name: "Ganda" },
    Entry { code: "lui", name: "Luiseno" },
    Entry { code: "lun", name: "Lunda" },
    Entry { code: "luo", name: "Luo (Kenya and Tanzania)" },
    Entry { code: "lus", name: "Lushai" },
    Entry { code: "mad", name: "Madurese" },
    Entry { code: "mag", name: "Magahi" },
    Entry { code: "mah", name: "Marshallese" },
    Entry { code: "mai", name: "Maithili" },
    Entry { code: "mak", name: "Makasar" },
    Entry { code: "mal", name: "Malayalam" },
    Entry { code: "man", name: "Mandingo" },
    Entry { code: "map", name: "Austronesian languages" },
    Entry { code: "mar", name: "Marathi" },
    Entry { code: "mas", name: "Masai" },
    Entry { code: "mdf", name: "Moksha" },
    Entry { code: "mdr", name: "Mandar" },
    Entry { code: "men", name: "Mende" },
    Entry { code: "mga", name: "Irish, Middle (900-1200)" },
    Entry { code: "mic", name: "Mi'kmaq" },
    Entry { code: "min", name: "Minangkabau" },
    Entry { code: "mis", name: "Uncoded languages" },
    Entry { code: "mkd", name: "Macedonian" },
    Entry { code: "mkh", name: "Mon-Khmer languages" },
    Entry { code: "mlg", name: "Malagasy" },
    Entry { code: "mlt", name: "Maltese" },
    Entry { code: "mnc", name: "Manchu" },
    Entry { code: "mni", name: "Manipuri" },
    Entry { code: "mno", name: "Manobo languages" },
    Entry { code: "moh", name: "Mohawk" },
    Entry { code: "mon", name: "Mongolian" },
    Entry { code: "mos", name: "Mossi" },
    Entry { code: "mri", name: "Maori" },
    Entry { code: "msa", name: "Malay" },
    Entry { code: "mul", name: "Multiple languages" },
    Entry { code: "mun", name: "Munda languages" },
    Entry { code: "mus", name: "Creek" },
    Entry { code: "mwl", name: "Mirandese" },
    Entry { code: "mwr", name: "Marwari" },
    Entry { code: "mya", name: "Burmese" },
    Entry { code: "myn", name: "Mayan languages" },
    Entry { code: "myv", name: "Erzya" },
    Entry { code: "nah", name: "Nahuatl languages" },
    Entry { code: "nai", name: "North American Indian languages" },
    Entry { code: "nap", name: "Neapolitan" },
    Entry { code: "nau", name: "Nauru" },
    Entry { code: "nav", name: "Navajo" },
    Entry { code: "nbl", name: "Ndebele, South" },
    Entry { code: "nde", name: "Ndebele, North" },
    Entry { code: "ndo", name: "Ndonga" },
    Entry { code: "nds", name: "Low German" },
    Entry { code: "nep", name: "Nepali" },
    Entry { code: "new", name: "Nepal Bhasa" },
    Entry { code: "nia", name: "Nias" },
    Entry { code: "nic", name: "Niger-Kordofanian languages" },
    Entry { code: "niu", name: "Niuean" },
    Entry { code: "nld", name: "Dutch" },
    Entry { code: "nno", name: "Norwegian Nynorsk" },
    Entry { code: "nob", name: "Bokmål, Norwegian" },
    Entry { code: "nog", name: "Nogai" },
    Entry { code: "non", name: "Norse, Old" },
    Entry { code: "nor", name: "Norwegian" },
    Entry { code: "nqo", name: "N'Ko" },
    Entry { code: "nso", name: "Pedi" },
    Entry { code: "nub", name: "Nubian languages" },
    Entry { code: "nwc", name: "Classical Newari" },
    Entry { code: "nya", name: "Chichewa" },
    Entry { code: "nym", name: "Nyamwezi" },
    Entry { code: "nyn", name: "Nyankole" },
    Entry { code: "nyo", name: "Nyoro" },
    Entry { code: "nzi", name: "Nzima" },
    Entry { code: "oci", name: "Occitan (post 1500)" },
    Entry { code: "oji", name: "Ojibwa" },
    Entry { code: "ori", name: "Oriya" },
    Entry { code: "orm", name: "Oromo" },
    Entry { code: "osa", name: "Osage" },
    Entry { code: "oss", name: "Ossetian" },
    Entry { code: "ota", name: "Turkish, Ottoman (1500-1928)" },
    Entry { code: "oto", name: "Otomian languages" },
    Entry { code: "paa", name: "Papuan languages" },
    Entry { code: "pag", name: "Pangasinan" },
    Entry { code: "pal", name: "Pahlavi" },
    Entry { code: "pam", name: "Pampanga" },
    Entry { code: "pan", name: "Panjabi" },
    Entry { code: "pap", name: "Papiamento" },
    Entry { code: "pau", name: "Palauan" },
    Entry { code: "peo", name: "Persian, Old (ca.600-400 B.C.)" },
    Entry { code: "phi", name: "Philippine languages" },
    Entry { code: "phn", name: "Phoenician" },
    Entry { code: "pli", name: "Pali" },
    Entry { code: "pol", name: "Polish" },
    Entry { code: "pon", name: "Pohnpeian" },
    Entry { code: "por", name: "Portuguese" },
    Entry { code: "pra", name: "Prakrit languages" },
    Entry { code: "pro", name: "Provençal, Old (to 1500)" },
    Entry { code: "pus", name: "Pushto" },
    Entry { code: "que", name: "Quechua" },
    Entry { code: "raj", name: "Rajasthani" },
    Entry { code: "rap", name: "Rapanui" },
    Entry { code: "rar", name: "Rarotongan" },
    Entry { code: "roa", name: "Romance languages" },
    Entry { code: "roh", name: "Romansh" },
    Entry { code: "rom", name: "Romany" },
    Entry { code: "ron", name: "Romanian" },
    Entry { code: "run", name: "Rundi" },
    Entry { code: "rup", name: "Aromanian" },
    Entry { code: "rus", name: "Russian" },
    Entry { code: "sad", name: "Sandawe" },
    Entry { code: "sag", name: "Sango" },
    Entry { code: "sah", name: "Yakut" },
    Entry { code: "sai", name: "South American Indian languages" },
    Entry { code: "sal", name: "Salishan languages" },
    Entry { code: "sam", name: "Samaritan Aramaic" },
    Entry { code: "san", name: "Sanskrit" },
    Entry { code: "sas", name: "Sasak" },
    Entry { code: "sat", name: "Santali" },
    Entry { code: "scn", name: "Sicilian" },
    Entry { code: "sco", name: "Scots" },
    Entry { code: "sel", name: "Selkup" },
    Entry { code: "sem", name: "Semitic languages" },
    Entry { code: "sga", name: "Irish, Old (to 900)" },
    Entry { code: "sgn", name: "Sign Languages" },
    Entry { code: "shn", name: "Shan" },
    Entry { code: "sid", name: "Sidamo" },
    Entry { code: "sin", name: "Sinhala" },
    Entry { code: "sio", name: "Siouan languages" },
    Entry { code: "sit", name: "Sino-Tibetan languages" },
    Entry { code: "sla", name: "Slavic languages" },
    Entry { code: "slk", name: "Slovak" },
    Entry { code: "slv", name: "Slovenian" },
    Entry { code: "sma", name: "Southern Sami" },
    Entry { code: "sme", name: "Northern Sami" },
    Entry { code: "smi", name: "Sami languages" },
    Entry { code: "smj", name: "Lule Sami" },
    Entry { code: "smn", name: "Inari Sami" },
    Entry { code: "smo", name: "Samoan" },
    Entry { code: "sms", name: "Skolt Sami" },
    Entry { code: "sna", name: "Shona" },
    Entry { code: "snd", name: "Sindhi" },
    Entry { code: "snk", name: "Soninke" },
    Entry { code: "sog", name: "Sogdian" },
    Entry { code: "som", name: "Somali" },
    Entry { code: "son", name: "Songhai languages" },
    Entry { code: "sot", name: "Sotho, Southern" },
    Entry { code: "spa", name: "Spanish" },
    Entry { code: "sqi", name: "Albanian" },
    Entry { code: "srd", name: "Sardinian" },
    Entry { code: "srn", name: "Sranan Tongo" },
    Entry { code: "srp", name: "Serbian" },
    Entry { code: "srr", name: "Serer" },
    Entry { code: "ssa", name: "Nilo-Saharan languages" },
    Entry { code: "ssw", name: "Swati" },
    Entry { code: "suk", name: "Sukuma" },
    Entry { code: "sun", name: "Sundanese" },
    Entry { code: "sus", name: "Susu" },
    Entry { code: "sux", name: "Sumerian" },
    Entry { code: "swa", name: "Swahili" },
    Entry { code: "swe", name: "Swedish" },
    Entry { code: "syc", name: "Classical Syriac" },
    Entry { code: "syr", name: "Syriac" },
    Entry { code: "tah", name: "Tahitian" },
    Entry { code: "tai", name: "Tai languages" },
    Entry { code: "tam", name: "Tamil" },
    Entry { code: "tat", name: "Tatar" },
    Entry { code: "tel", name: "Telugu" },
    Entry { code: "tem", name: "Timne" },
    Entry { code: "ter", name: "Tereno" },
    Entry { code: "tet", name: "Tetum" },
    Entry { code: "tgk", name: "Tajik" },
    Entry { code: "tgl", name: "Tagalog" },
    Entry { code: "tha", name: "Thai" },
    Entry { code: "tig", name: "Tigre" },
    Entry { code: "tir", name: "Tigrinya" },
    Entry { code: "tiv", name: "Tiv" },
    Entry { code: "tkl", name: "Tokelau" },
    Entry { code: "tlh", name: "Klingon" },
    Entry { code: "tli", name: "Tlingit" },
    Entry { code: "tmh", name: "Tamashek" },
    Entry { code: "tog", name: "Tonga (Nyasa)" },
    Entry { code: "ton", name: "Tonga (Tonga Islands)" },
    Entry { code: "tpi", name: "Tok Pisin" },
    Entry { code: "tsi", name: "Tsimshian" },
    Entry { code: "tsn", name: "Tswana" },
    Entry { code: "tso", name: "Tsonga" },
    Entry { code: "tuk", name: "Turkmen" },
    Entry { code: "tum", name: "Tumbuka" },
    Entry { code: "tup", name: "Tupi languages" },
    Entry { code: "tur", name: "Turkish" },
    Entry { code: "tut", name: "Altaic languages" },
    Entry { code: "tvl", name: "Tuvalu" },
    Entry { code: "twi", name: "Twi" },
    Entry { code: "tyv", name: "Tuvinian" },
    Entry { code: "udm", name: "Udmurt" },
    Entry { code: "uga", name: "Ugaritic" },
    Entry { code: "uig", name: "Uighur" },
    Entry { code: "ukr", name: "Ukrainian" },
    Entry { code: "umb", name: "Umbundu" },
    Entry { code: "und", name: "Undetermined" },
    Entry { code: "urd", name: "Urdu" },
    Entry { code: "uzb", name: "Uzbek" },
    Entry { code: "vai", name: "Vai" },
    Entry { code: "ven", name: "Venda" },
    Entry { code: "vie", name: "Vietnamese" },
    Entry { code: "vol", name: "Volapük" },
    Entry { code: "vot", name: "Votic" },
    Entry { code: "wak", name: "Wakashan languages" },
    Entry { code: "wal", name: "Wolaitta" },
    Entry { code: "war", name: "Waray" },
    Entry { code: "was", name: "Washo" },
    Entry { code: "wen", name: "Sorbian languages" },
    Entry { code: "wln", name: "Walloon" },
    Entry { code: "wol", name: "Wolof" },
    Entry { code: "xal", name: "Kalmyk" },
    Entry { code: "xho", name: "Xhosa" },
    Entry { code: "yao", name: "Yao" },
    Entry { code: "yap", name: "Yapese" },
    Entry { code: "yid", name: "Yiddish" },
    Entry { code: "yor", name: "Yoruba" },
    Entry { code: "ypk", name: "Yupik languages" },
    Entry { code: "zap", name: "Zapotec" },
    Entry { code: "zbl", name: "Blissymbols" },
    Entry { code: "zen", name: "Zenaga" },
    Entry { code: "zgh", name: "Standard Moroccan Tamazight" },
    Entry { code: "zha", name: "Zhuang" },
    Entry { code: "zho", name: "Chinese" },
    Entry { code: "znd", name: "Zande languages" },
    Entry { code: "zul", name: "Zulu" },
    Entry { code: "zun", name: "Zuni" },
    Entry { code: "zxx", name: "No linguistic content" },
    Entry { code: "zza", name: "Zaza" },
];

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn looks_up_common_codes() {
    let m = lookup("eng").unwrap();
    assert_eq!(m.canonical, "eng");
    assert_eq!(m.name, "English");
    let m = lookup("jpn").unwrap();
    assert_eq!(m.canonical, "jpn");
    assert_eq!(m.name, "Japanese");
  }

  #[test]
  fn case_insensitive() {
    assert_eq!(lookup("ENG").unwrap().canonical, "eng");
    assert_eq!(lookup("Eng").unwrap().canonical, "eng");
    assert_eq!(lookup("eNG").unwrap().canonical, "eng");
  }

  #[test]
  fn whitespace_tolerant() {
    assert_eq!(lookup("  eng  ").unwrap().canonical, "eng");
  }

  #[test]
  fn rejects_wrong_length() {
    assert!(lookup("en").is_none());
    assert!(lookup("eng2").is_none());
    assert!(lookup("").is_none());
  }

  #[test]
  fn rejects_unknown_code() {
    assert!(lookup("xyz").is_none());
    assert!(lookup("zzz").is_none());
  }

  #[test]
  fn bib_codes_map_to_term_canonical() {
    let m = lookup("fre").unwrap();
    assert_eq!(m.canonical, "fra");
    assert_eq!(m.name, "French");
    let m = lookup("ger").unwrap();
    assert_eq!(m.canonical, "deu");
    assert_eq!(m.name, "German");
    let m = lookup("chi").unwrap();
    assert_eq!(m.canonical, "zho");
    assert_eq!(m.name, "Chinese");
  }

  #[test]
  fn is_valid_matches_lookup() {
    assert!(is_valid("eng"));
    assert!(is_valid("fre")); // bib alias is still valid input
    assert!(is_valid("FRE"));
    assert!(!is_valid("zzz"));
    assert!(!is_valid("en"));
  }

  #[test]
  fn special_codes_are_present() {
    assert!(is_valid(UND));
    assert!(is_valid(ZXX));
    assert_eq!(lookup(UND).unwrap().name, "Undetermined");
    assert_eq!(lookup(ZXX).unwrap().name, "No linguistic content");
  }

  #[test]
  fn table_is_sorted_by_code() {
    for window in TABLE.windows(2) {
      assert!(
        window[0].code < window[1].code,
        "ISO 639-2 table not sorted: {} >= {}",
        window[0].code,
        window[1].code
      );
    }
  }

  #[test]
  fn every_bib_alias_resolves_to_a_present_term_code() {
    for (bib, term) in BIB_T_PAIRS {
      assert!(
        TABLE.binary_search_by_key(term, |e| e.code).is_ok(),
        "term code {} (alias of {}) not present in TABLE",
        term,
        bib
      );
    }
  }

  #[test]
  fn table_size_is_in_expected_range() {
    // ISO 639-2 has ~487 distinct terminologic codes.  We allow some slack
    // because the registry adds a code now and again.
    assert!(TABLE.len() >= 480, "table shrunk: {}", TABLE.len());
    assert!(TABLE.len() <= 520, "table is suspiciously large: {}", TABLE.len());
  }

  #[test]
  fn alpha2_table_is_sorted_for_binary_search() {
    for window in ALPHA2_TO_ALPHA3.windows(2) {
      assert!(
        window[0].0 < window[1].0,
        "alpha-2 table not sorted: {} >= {}",
        window[0].0,
        window[1].0
      );
    }
  }

  #[test]
  fn alpha2_maps_common_languages() {
    assert_eq!(alpha2_to_alpha3("en"), Some("eng"));
    assert_eq!(alpha2_to_alpha3("ja"), Some("jpn"));
    assert_eq!(alpha2_to_alpha3("EN"), Some("eng")); // case-insensitive
    assert_eq!(alpha2_to_alpha3(" pt "), Some("por")); // whitespace-tolerant
  }

  #[test]
  fn alpha2_maps_bibliographic_targets_then_lookup_canonicalises() {
    // "de" → "ger" (bib); lookup resolves to terminologic "deu".
    assert_eq!(alpha2_to_alpha3("de"), Some("ger"));
    assert_eq!(lookup("ger").unwrap().canonical, "deu");
    assert_eq!(alpha2_to_alpha3("zh"), Some("chi"));
    assert_eq!(lookup("chi").unwrap().canonical, "zho");
  }

  #[test]
  fn alpha2_rejects_wrong_length_and_unknown() {
    assert_eq!(alpha2_to_alpha3("eng"), None);
    assert_eq!(alpha2_to_alpha3("e"), None);
    assert_eq!(alpha2_to_alpha3("zz"), None);
    assert_eq!(alpha2_to_alpha3(""), None);
  }

  #[test]
  fn every_alpha2_target_is_resolvable() {
    // `sh → hbs` (Serbo-Croatian) is the one alpha-2 whose target is a
    // 639-3-only macrolanguage; mkvtoolnix flags `hbs` as not part of
    // 639-2, so our 639-2 table omits it and `from_ietf("sh")` falls back
    // to `und`.  Every other alpha-2 target must resolve.
    for (a2, a3) in ALPHA2_TO_ALPHA3 {
      if *a3 == "hbs" {
        continue;
      }
      assert!(
        lookup(a3).is_some(),
        "alpha-2 {} maps to {} which lookup() cannot resolve",
        a2,
        a3
      );
    }
  }
}
