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

//! The MKV language table, generated once from mkvtoolnix's
//! `src/common/iso639_language_list.cpp`: ISO 639-1-backed languages plus the
//! Matroska-relevant generic entries. The label format is mkvtoolnix's:
//! English name, then the ISO 639 codes in parentheses.

use super::MkvLanguage;

pub(super) const ENTRIES: [MkvLanguage; 186] = [
  MkvLanguage {
    code: "und",
    label: "Undetermined (und)",
  },
  MkvLanguage {
    code: "mul",
    label: "Multiple languages (mul)",
  },
  MkvLanguage {
    code: "abk",
    label: "Abkhazian (ab; abk)",
  },
  MkvLanguage {
    code: "aar",
    label: "Afar (aa; aar)",
  },
  MkvLanguage {
    code: "afr",
    label: "Afrikaans (af; afr)",
  },
  MkvLanguage {
    code: "aka",
    label: "Akan (ak; aka)",
  },
  MkvLanguage {
    code: "alb",
    label: "Albanian (sq; alb)",
  },
  MkvLanguage {
    code: "amh",
    label: "Amharic (am; amh)",
  },
  MkvLanguage {
    code: "ara",
    label: "Arabic (ar; ara)",
  },
  MkvLanguage {
    code: "arg",
    label: "Aragonese (an; arg)",
  },
  MkvLanguage {
    code: "arm",
    label: "Armenian (hy; arm)",
  },
  MkvLanguage {
    code: "asm",
    label: "Assamese (as; asm)",
  },
  MkvLanguage {
    code: "ava",
    label: "Avaric (av; ava)",
  },
  MkvLanguage {
    code: "ave",
    label: "Avestan (ae; ave)",
  },
  MkvLanguage {
    code: "aym",
    label: "Aymara (ay; aym)",
  },
  MkvLanguage {
    code: "aze",
    label: "Azerbaijani (az; aze)",
  },
  MkvLanguage {
    code: "bam",
    label: "Bambara (bm; bam)",
  },
  MkvLanguage {
    code: "bak",
    label: "Bashkir (ba; bak)",
  },
  MkvLanguage {
    code: "baq",
    label: "Basque (eu; baq)",
  },
  MkvLanguage {
    code: "bel",
    label: "Belarusian (be; bel)",
  },
  MkvLanguage {
    code: "ben",
    label: "Bengali (bn; ben)",
  },
  MkvLanguage {
    code: "bis",
    label: "Bislama (bi; bis)",
  },
  MkvLanguage {
    code: "bos",
    label: "Bosnian (bs; bos)",
  },
  MkvLanguage {
    code: "bre",
    label: "Breton (br; bre)",
  },
  MkvLanguage {
    code: "bul",
    label: "Bulgarian (bg; bul)",
  },
  MkvLanguage {
    code: "bur",
    label: "Burmese (my; bur)",
  },
  MkvLanguage {
    code: "cat",
    label: "Catalan (ca; cat)",
  },
  MkvLanguage {
    code: "cha",
    label: "Chamorro (ch; cha)",
  },
  MkvLanguage {
    code: "che",
    label: "Chechen (ce; che)",
  },
  MkvLanguage {
    code: "chi",
    label: "Chinese (zh; chi)",
  },
  MkvLanguage {
    code: "chu",
    label: "Church Slavic (cu; chu)",
  },
  MkvLanguage {
    code: "chv",
    label: "Chuvash (cv; chv)",
  },
  MkvLanguage {
    code: "cor",
    label: "Cornish (kw; cor)",
  },
  MkvLanguage {
    code: "cos",
    label: "Corsican (co; cos)",
  },
  MkvLanguage {
    code: "cre",
    label: "Cree (cr; cre)",
  },
  MkvLanguage {
    code: "hrv",
    label: "Croatian (hr; hrv)",
  },
  MkvLanguage {
    code: "cze",
    label: "Czech (cs; cze)",
  },
  MkvLanguage {
    code: "dan",
    label: "Danish (da; dan)",
  },
  MkvLanguage {
    code: "div",
    label: "Dhivehi (dv; div)",
  },
  MkvLanguage {
    code: "dut",
    label: "Dutch (nl; dut)",
  },
  MkvLanguage {
    code: "dzo",
    label: "Dzongkha (dz; dzo)",
  },
  MkvLanguage {
    code: "eng",
    label: "English (en; eng)",
  },
  MkvLanguage {
    code: "epo",
    label: "Esperanto (eo; epo)",
  },
  MkvLanguage {
    code: "est",
    label: "Estonian (et; est)",
  },
  MkvLanguage {
    code: "ewe",
    label: "Ewe (ee; ewe)",
  },
  MkvLanguage {
    code: "fao",
    label: "Faroese (fo; fao)",
  },
  MkvLanguage {
    code: "fij",
    label: "Fijian (fj; fij)",
  },
  MkvLanguage {
    code: "fin",
    label: "Finnish (fi; fin)",
  },
  MkvLanguage {
    code: "fre",
    label: "French (fr; fre)",
  },
  MkvLanguage {
    code: "ful",
    label: "Fulah (ff; ful)",
  },
  MkvLanguage {
    code: "glg",
    label: "Galician (gl; glg)",
  },
  MkvLanguage {
    code: "lug",
    label: "Ganda (lg; lug)",
  },
  MkvLanguage {
    code: "geo",
    label: "Georgian (ka; geo)",
  },
  MkvLanguage {
    code: "ger",
    label: "German (de; ger)",
  },
  MkvLanguage {
    code: "gre",
    label: "Greek (modern, 1453-) (el; gre)",
  },
  MkvLanguage {
    code: "grn",
    label: "Guarani (gn; grn)",
  },
  MkvLanguage {
    code: "guj",
    label: "Gujarati (gu; guj)",
  },
  MkvLanguage {
    code: "hat",
    label: "Haitian (ht; hat)",
  },
  MkvLanguage {
    code: "hau",
    label: "Hausa (ha; hau)",
  },
  MkvLanguage {
    code: "heb",
    label: "Hebrew (he; heb)",
  },
  MkvLanguage {
    code: "her",
    label: "Herero (hz; her)",
  },
  MkvLanguage {
    code: "hin",
    label: "Hindi (hi; hin)",
  },
  MkvLanguage {
    code: "hmo",
    label: "Hiri Motu (ho; hmo)",
  },
  MkvLanguage {
    code: "hun",
    label: "Hungarian (hu; hun)",
  },
  MkvLanguage {
    code: "ice",
    label: "Icelandic (is; ice)",
  },
  MkvLanguage {
    code: "ido",
    label: "Ido (io; ido)",
  },
  MkvLanguage {
    code: "ibo",
    label: "Igbo (ig; ibo)",
  },
  MkvLanguage {
    code: "ind",
    label: "Indonesian (id; ind)",
  },
  MkvLanguage {
    code: "ina",
    label: "Interlingua (International Auxiliary Language Association) (ia; ina)",
  },
  MkvLanguage {
    code: "ile",
    label: "Interlingue (ie; ile)",
  },
  MkvLanguage {
    code: "iku",
    label: "Inuktitut (iu; iku)",
  },
  MkvLanguage {
    code: "ipk",
    label: "Inupiaq (ik; ipk)",
  },
  MkvLanguage {
    code: "gle",
    label: "Irish (ga; gle)",
  },
  MkvLanguage {
    code: "ita",
    label: "Italian (it; ita)",
  },
  MkvLanguage {
    code: "jpn",
    label: "Japanese (ja; jpn)",
  },
  MkvLanguage {
    code: "jav",
    label: "Javanese (jv; jav)",
  },
  MkvLanguage {
    code: "kal",
    label: "Kalaallisut (kl; kal)",
  },
  MkvLanguage {
    code: "kan",
    label: "Kannada (kn; kan)",
  },
  MkvLanguage {
    code: "kau",
    label: "Kanuri (kr; kau)",
  },
  MkvLanguage {
    code: "kas",
    label: "Kashmiri (ks; kas)",
  },
  MkvLanguage {
    code: "kaz",
    label: "Kazakh (kk; kaz)",
  },
  MkvLanguage {
    code: "khm",
    label: "Khmer (km; khm)",
  },
  MkvLanguage {
    code: "kik",
    label: "Kikuyu (ki; kik)",
  },
  MkvLanguage {
    code: "kin",
    label: "Kinyarwanda (rw; kin)",
  },
  MkvLanguage {
    code: "kir",
    label: "Kirghiz (ky; kir)",
  },
  MkvLanguage {
    code: "kom",
    label: "Komi (kv; kom)",
  },
  MkvLanguage {
    code: "kon",
    label: "Kongo (kg; kon)",
  },
  MkvLanguage {
    code: "kor",
    label: "Korean (ko; kor)",
  },
  MkvLanguage {
    code: "kua",
    label: "Kuanyama (kj; kua)",
  },
  MkvLanguage {
    code: "kur",
    label: "Kurdish (ku; kur)",
  },
  MkvLanguage {
    code: "lao",
    label: "Lao (lo; lao)",
  },
  MkvLanguage {
    code: "lat",
    label: "Latin (la; lat)",
  },
  MkvLanguage {
    code: "lav",
    label: "Latvian (lv; lav)",
  },
  MkvLanguage {
    code: "lim",
    label: "Limburgan (li; lim)",
  },
  MkvLanguage {
    code: "lin",
    label: "Lingala (ln; lin)",
  },
  MkvLanguage {
    code: "lit",
    label: "Lithuanian (lt; lit)",
  },
  MkvLanguage {
    code: "lub",
    label: "Luba-Katanga (lu; lub)",
  },
  MkvLanguage {
    code: "ltz",
    label: "Luxembourgish (lb; ltz)",
  },
  MkvLanguage {
    code: "mac",
    label: "Macedonian (mk; mac)",
  },
  MkvLanguage {
    code: "mlg",
    label: "Malagasy (mg; mlg)",
  },
  MkvLanguage {
    code: "may",
    label: "Malay (macrolanguage) (ms; may)",
  },
  MkvLanguage {
    code: "mal",
    label: "Malayalam (ml; mal)",
  },
  MkvLanguage {
    code: "mlt",
    label: "Maltese (mt; mlt)",
  },
  MkvLanguage {
    code: "glv",
    label: "Manx (gv; glv)",
  },
  MkvLanguage {
    code: "mao",
    label: "Maori (mi; mao)",
  },
  MkvLanguage {
    code: "mar",
    label: "Marathi (mr; mar)",
  },
  MkvLanguage {
    code: "mah",
    label: "Marshallese (mh; mah)",
  },
  MkvLanguage {
    code: "mon",
    label: "Mongolian (mn; mon)",
  },
  MkvLanguage {
    code: "nau",
    label: "Nauru (na; nau)",
  },
  MkvLanguage {
    code: "nav",
    label: "Navajo (nv; nav)",
  },
  MkvLanguage {
    code: "ndo",
    label: "Ndonga (ng; ndo)",
  },
  MkvLanguage {
    code: "nep",
    label: "Nepali (macrolanguage) (ne; nep)",
  },
  MkvLanguage {
    code: "nde",
    label: "North Ndebele (nd; nde)",
  },
  MkvLanguage {
    code: "sme",
    label: "Northern Sami (se; sme)",
  },
  MkvLanguage {
    code: "nor",
    label: "Norwegian (no; nor)",
  },
  MkvLanguage {
    code: "nob",
    label: "Norwegian Bokmål (nb; nob)",
  },
  MkvLanguage {
    code: "nno",
    label: "Norwegian Nynorsk (nn; nno)",
  },
  MkvLanguage {
    code: "nya",
    label: "Nyanja (ny; nya)",
  },
  MkvLanguage {
    code: "oci",
    label: "Occitan (post 1500) (oc; oci)",
  },
  MkvLanguage {
    code: "oji",
    label: "Ojibwa (oj; oji)",
  },
  MkvLanguage {
    code: "ori",
    label: "Oriya (macrolanguage) (or; ori)",
  },
  MkvLanguage {
    code: "orm",
    label: "Oromo (om; orm)",
  },
  MkvLanguage {
    code: "oss",
    label: "Ossetian (os; oss)",
  },
  MkvLanguage {
    code: "pli",
    label: "Pali (pi; pli)",
  },
  MkvLanguage {
    code: "pan",
    label: "Panjabi (pa; pan)",
  },
  MkvLanguage {
    code: "per",
    label: "Persian (fa; per)",
  },
  MkvLanguage {
    code: "pol",
    label: "Polish (pl; pol)",
  },
  MkvLanguage {
    code: "por",
    label: "Portuguese (pt; por)",
  },
  MkvLanguage {
    code: "pus",
    label: "Pushto (ps; pus)",
  },
  MkvLanguage {
    code: "que",
    label: "Quechua (qu; que)",
  },
  MkvLanguage {
    code: "rum",
    label: "Romanian (ro; rum)",
  },
  MkvLanguage {
    code: "roh",
    label: "Romansh (rm; roh)",
  },
  MkvLanguage {
    code: "run",
    label: "Rundi (rn; run)",
  },
  MkvLanguage {
    code: "rus",
    label: "Russian (ru; rus)",
  },
  MkvLanguage {
    code: "smo",
    label: "Samoan (sm; smo)",
  },
  MkvLanguage {
    code: "sag",
    label: "Sango (sg; sag)",
  },
  MkvLanguage {
    code: "san",
    label: "Sanskrit (sa; san)",
  },
  MkvLanguage {
    code: "srd",
    label: "Sardinian (sc; srd)",
  },
  MkvLanguage {
    code: "gla",
    label: "Scottish Gaelic (gd; gla)",
  },
  MkvLanguage {
    code: "srp",
    label: "Serbian (sr; srp)",
  },
  MkvLanguage {
    code: "hbs",
    label: "Serbo-Croatian (sh; hbs)",
  },
  MkvLanguage {
    code: "sna",
    label: "Shona (sn; sna)",
  },
  MkvLanguage {
    code: "iii",
    label: "Sichuan Yi (ii; iii)",
  },
  MkvLanguage {
    code: "snd",
    label: "Sindhi (sd; snd)",
  },
  MkvLanguage {
    code: "sin",
    label: "Sinhala (si; sin)",
  },
  MkvLanguage {
    code: "slo",
    label: "Slovak (sk; slo)",
  },
  MkvLanguage {
    code: "slv",
    label: "Slovenian (sl; slv)",
  },
  MkvLanguage {
    code: "som",
    label: "Somali (so; som)",
  },
  MkvLanguage {
    code: "nbl",
    label: "South Ndebele (nr; nbl)",
  },
  MkvLanguage {
    code: "sot",
    label: "Southern Sotho (st; sot)",
  },
  MkvLanguage {
    code: "spa",
    label: "Spanish (es; spa)",
  },
  MkvLanguage {
    code: "sun",
    label: "Sundanese (su; sun)",
  },
  MkvLanguage {
    code: "swa",
    label: "Swahili (macrolanguage) (sw; swa)",
  },
  MkvLanguage {
    code: "ssw",
    label: "Swati (ss; ssw)",
  },
  MkvLanguage {
    code: "swe",
    label: "Swedish (sv; swe)",
  },
  MkvLanguage {
    code: "tgl",
    label: "Tagalog (tl; tgl)",
  },
  MkvLanguage {
    code: "tah",
    label: "Tahitian (ty; tah)",
  },
  MkvLanguage {
    code: "tgk",
    label: "Tajik (tg; tgk)",
  },
  MkvLanguage {
    code: "tam",
    label: "Tamil (ta; tam)",
  },
  MkvLanguage {
    code: "tat",
    label: "Tatar (tt; tat)",
  },
  MkvLanguage {
    code: "tel",
    label: "Telugu (te; tel)",
  },
  MkvLanguage {
    code: "tha",
    label: "Thai (th; tha)",
  },
  MkvLanguage {
    code: "tib",
    label: "Tibetan (bo; tib)",
  },
  MkvLanguage {
    code: "tir",
    label: "Tigrinya (ti; tir)",
  },
  MkvLanguage {
    code: "ton",
    label: "Tonga (Tonga Islands) (to; ton)",
  },
  MkvLanguage {
    code: "tso",
    label: "Tsonga (ts; tso)",
  },
  MkvLanguage {
    code: "tsn",
    label: "Tswana (tn; tsn)",
  },
  MkvLanguage {
    code: "tur",
    label: "Turkish (tr; tur)",
  },
  MkvLanguage {
    code: "tuk",
    label: "Turkmen (tk; tuk)",
  },
  MkvLanguage {
    code: "twi",
    label: "Twi (tw; twi)",
  },
  MkvLanguage {
    code: "uig",
    label: "Uighur (ug; uig)",
  },
  MkvLanguage {
    code: "ukr",
    label: "Ukrainian (uk; ukr)",
  },
  MkvLanguage {
    code: "urd",
    label: "Urdu (ur; urd)",
  },
  MkvLanguage {
    code: "uzb",
    label: "Uzbek (uz; uzb)",
  },
  MkvLanguage {
    code: "ven",
    label: "Venda (ve; ven)",
  },
  MkvLanguage {
    code: "vie",
    label: "Vietnamese (vi; vie)",
  },
  MkvLanguage {
    code: "vol",
    label: "Volapük (vo; vol)",
  },
  MkvLanguage {
    code: "wln",
    label: "Walloon (wa; wln)",
  },
  MkvLanguage {
    code: "wel",
    label: "Welsh (cy; wel)",
  },
  MkvLanguage {
    code: "fry",
    label: "Western Frisian (fy; fry)",
  },
  MkvLanguage {
    code: "wol",
    label: "Wolof (wo; wol)",
  },
  MkvLanguage {
    code: "xho",
    label: "Xhosa (xh; xho)",
  },
  MkvLanguage {
    code: "yid",
    label: "Yiddish (yi; yid)",
  },
  MkvLanguage {
    code: "yor",
    label: "Yoruba (yo; yor)",
  },
  MkvLanguage {
    code: "zha",
    label: "Zhuang (za; zha)",
  },
  MkvLanguage {
    code: "zul",
    label: "Zulu (zu; zul)",
  },
];
