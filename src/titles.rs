//! Readable titles for the game's cue names, e.g. `BGM_LD_FF09_NOT_ALONE`
//! becomes "FF9 Not Alone". Names the rules get wrong can be fixed by hand
//! in the config's `[titles]` table.

/// Whole names (after the `BGM_PP_` / `BGM_LD_` prefix) that the rules
/// cannot work out: translated Japanese, and odd one-offs.
const NAMES: &[(&str, &str)] = &[
    ("MYPAGE_NORMAL1", "Home Menu"),
    ("BATTLEEND", "Victory"),
    ("MAINTHEME_WITH_END", "Main Theme (with Ending)"),
    ("FFBE_BGM_RELAX", "FFBE Relax"),
    ("SIRENNOUTA", "Siren's Song"),
    ("SIRENNOUTA_INST", "Siren's Song (Instrumental)"),
    ("RURUKA_INORINOUTA", "Ruruka: Song of Prayer"),
    ("RURUKA_INORINOUTA_EDIT", "Ruruka: Song of Prayer (Edit)"),
    ("RURUKA_INORINOUTA_INST", "Ruruka: Song of Prayer (Instrumental)"),
    ("700NENMAE", "700 Years Ago"),
    ("700NENMAE_INST", "700 Years Ago (Instrumental)"),
    ("RAINNOKAKUSEI", "Rain's Awakening"),
    ("RAINNOKAKUSEI_A", "Rain's Awakening (A)"),
    ("RAINNOKAKUSEI_B", "Rain's Awakening (B)"),
    ("SEKAISYUENNOZYOKYOKU", "Prelude to the End of the World"),
    ("VS_GENZYU", "Vs. Summons"),
    ("FINA_SEISHINSEKAI", "Fina: Spirit World"),
    ("FINA_SEISHINSEKAI_INST", "Fina: Spirit World (Instrumental)"),
    ("ROKUMEIKETSU_THEMA", "Rokumeiketsu Theme"),
    ("EVENT_KANDOU01", "Event: Emotional 1"),
    ("EVENT_KANDOU02", "Event: Emotional 2"),
    ("EVENT_SHUGEKI", "Event: Raid"),
    ("TOWN_TEITO_ZOLDARD", "Town: Imperial Capital Zoldard"),
    ("BATTLE_VERITAS_TOKOYAMI", "Battle: Veritas of Eternal Darkness"),
    ("TOWN_F_VOICE_CUT", "Town: F (No Voice)"),
    ("C01_001_01_MOVIE", "Chapter 1 Movie"),
    ("LASTBOSS", "Last Boss"),
];

/// Run-together words in the cue names.
const WORDS: &[(&str, &str)] = &[
    ("MAINTHEME", "Main Theme"),
    ("NORMALBOSS", "Normal Boss"),
    ("MIDBOSS", "Mid Boss"),
    ("LASTBOSS", "Last Boss"),
    ("IMPERIALCAPITAL", "Imperial Capital"),
    ("PORTTOWN", "Port Town"),
    ("DESERTEDMETROPOLIS", "Deserted Metropolis"),
    ("DIFFERENTWORLD", "Different World"),
    ("DUNGENON", "Dungeon"),
    ("FIRMGROUND", "Firm Ground"),
    ("GHOSTSHIP", "Ghost Ship"),
    ("DESERTEDCASTLE", "Deserted Castle"),
    ("SIRENTOWER", "Siren Tower"),
    ("CRISTALTOWER", "Crystal Tower"),
    ("CRYSTALTOWER", "Crystal Tower"),
    ("GRANDNAVIGATION", "Grand Navigation"),
    ("VISIONGET", "Vision Obtained"),
    ("CHOCOBORACE", "Chocobo Race"),
    ("TIMBERTRAIN", "Timber Train"),
    ("ENDINGTHEME", "Ending Theme"),
    ("TINAS", "Tina's"),
    ("LIGHTNINGS", "Lightning's"),
    ("VANADIEL", "Vana'diel"),
    ("12ARMS", "12 Arms"),
    ("THEMA", "Theme"),
    ("WORLDMAP", "World Map"),
    ("OP", "Opening"),
];

/// Version tags at the end of a name, shown in brackets.
const TAGS: &[(&str, &str)] = &[
    ("FFPR", "Pixel Remaster"),
    ("SFC", "SNES"),
    ("PS", "PS1"),
    ("PS2", "PS2"),
    ("INST", "Instrumental"),
    ("VOCAL", "Vocal"),
    ("NOINTRO", "No Intro"),
    ("INTRO", "Intro"),
    ("NOLOOP", "No Loop"),
    ("EDIT", "Edit"),
    ("REPRISE", "Reprise"),
];

/// Leading words that name where a theme plays.
const CATEGORIES: &[(&str, &str)] = &[
    ("EVENT", "Event"),
    ("EV", "Event"),
    ("BATTLE", "Battle"),
    ("TOWN", "Town"),
    ("DUNGEON", "Dungeon"),
    ("DUNGENON", "Dungeon"),
    ("MAP", "Map"),
];

const SMALL: &[&str] = &["of", "the", "and", "to", "a", "with", "which", "no"];

fn lookup(table: &[(&str, &'static str)], key: &str) -> Option<&'static str> {
    table.iter().find(|(k, _)| *k == key).map(|(_, v)| *v)
}

/// "SWORDS" -> "Swords", "WORLD1" -> "World 1", "02" -> "2".
fn word(w: &str) -> String {
    if let Some(fixed) = lookup(WORDS, w) {
        return fixed.to_string();
    }
    if w.chars().all(|c| c.is_ascii_digit()) {
        return w.trim_start_matches('0').to_string().max("0".into());
    }
    // Trailing digits become their own number.
    let split = w.trim_end_matches(|c: char| c.is_ascii_digit()).len();
    let (text, num) = w.split_at(split);
    let mut out = String::new();
    if let Some(fixed) = lookup(WORDS, text) {
        out.push_str(fixed);
    } else {
        let mut chars = text.chars();
        if let Some(first) = chars.next() {
            out.push(first.to_ascii_uppercase());
            out.extend(chars.map(|c| c.to_ascii_lowercase()));
        }
    }
    if !num.is_empty() && !text.is_empty() {
        out.push(' ');
        out.push_str(num.trim_start_matches('0'));
    } else {
        out.push_str(num);
    }
    out
}

/// Joins words, keeping short joining words lower-case except at the start.
fn join(words: &[String]) -> String {
    words
        .iter()
        .enumerate()
        .map(|(i, w)| {
            let lower = w.to_lowercase();
            if i > 0 && SMALL.contains(&lower.as_str()) { lower } else { w.clone() }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

pub fn pretty(cue: &str) -> String {
    let name = cue.strip_prefix("BGM_").unwrap_or(cue);
    let name = ["PP_", "LD_", "ADV_"].iter().find_map(|p| name.strip_prefix(p)).unwrap_or(name);
    if let Some(fixed) = lookup(NAMES, name) {
        return fixed.to_string();
    }
    let mut tokens: Vec<&str> = name.split('_').filter(|t| !t.is_empty()).collect();

    // Version tags and take numbers ("_02", "_A") from the end.
    let mut tags = Vec::new();
    while tokens.len() > 1 {
        let last = tokens[tokens.len() - 1];
        if let Some(tag) = lookup(TAGS, last) {
            tags.insert(0, tag.to_string());
        } else if last.len() == 1 && last.chars().all(|c| c.is_ascii_uppercase()) && tokens.len() > 2 {
            tags.insert(0, last.to_string());
        } else if last.len() == 2 && last.chars().all(|c| c.is_ascii_digit()) {
            tags.insert(0, last.trim_start_matches('0').to_string());
        } else if last == "TRUE" {
            tags.insert(0, "True".to_string());
        } else {
            break;
        }
        tokens.pop();
    }

    // "FF09" -> "FF9", then the song name: "FF9 Not Alone".
    let mut prefix = String::new();
    if let Some(n) = tokens.first().and_then(|t| t.strip_prefix("FF")).filter(|n| n.chars().all(|c| c.is_ascii_digit()) && !n.is_empty()) {
        prefix = format!("FF{}", n.trim_start_matches('0'));
        tokens.remove(0);
    } else if let Some(part) = tokens.first().and_then(|t| match *t {
        "3RD" => Some("Part 3"),
        "4TH" => Some("Part 4"),
        _ => None,
    }) {
        // "3RD_BGM_3RD_KEEPERS" -> "Part 3: Keepers".
        prefix = format!("{part}:");
        tokens.remove(0);
        while matches!(tokens.first(), Some(&"BGM") | Some(&"3RD")) {
            tokens.remove(0);
        }
    } else if let Some(cat) = (tokens.len() > 1).then(|| lookup(CATEGORIES, tokens[0])).flatten() {
        prefix = format!("{cat}:");
        tokens.remove(0);
    } else if tokens.first() == Some(&"VS") {
        prefix = "Vs.".into();
        tokens.remove(0);
    } else if tokens.len() > 1
        && tokens[0].len() == 3
        && tokens[0].starts_with('M')
        && tokens[0][1..].chars().all(|c| c.is_ascii_digit())
    {
        // "M01_DISTANCE": a numbered song; the number adds nothing.
        tokens.remove(0);
    }

    // "LA01_005" is a scene code; keep it as one readable code, "LA01-005".
    if let Some(p) = tokens.iter().position(|t| t.len() == 4 && t.starts_with("LA")) {
        if let Some(next) = tokens.get(p + 1).filter(|n| n.chars().all(|c| c.is_ascii_digit())) {
            let code = format!("{}-{next}", tokens[p]);
            let mut words: Vec<String> = tokens[..p].iter().map(|t| word(t)).collect();
            words.push(code);
            words.extend(tokens[p + 2..].iter().map(|t| word(t)));
            let body = join(&words);
            return if prefix.is_empty() { body } else { format!("{prefix} {body}") };
        }
    }

    let words: Vec<String> = tokens.iter().map(|t| word(t)).collect();
    let mut title = match (prefix.is_empty(), words.is_empty()) {
        (true, _) => join(&words),
        (false, true) => prefix.trim_end_matches(':').to_string(),
        (false, false) => format!("{prefix} {}", join(&words)),
    };
    if !tags.is_empty() {
        title = format!("{title} ({})", tags.join(", "));
    }
    title
}

#[cfg(test)]
mod tests {
    use super::pretty;

    #[test]
    fn examples() {
        assert_eq!(pretty("BGM_LD_FF09_NOT_ALONE"), "FF9 Not Alone");
        assert_eq!(pretty("BGM_LD_FF06_TINAS_THEME_SFC"), "FF6 Tina's Theme (SNES)");
        assert_eq!(pretty("BGM_PP_TOWN_IMPERIALCAPITAL"), "Town: Imperial Capital");
        assert_eq!(pretty("BGM_PP_MAP_WORLD1"), "Map: World 1");
        assert_eq!(pretty("BGM_LD_FF05_02"), "FF5 (2)");
    }

    /// `JUKEBOX_LOG=<path to ffrs_jukebox.log> cargo test -- --nocapture all`
    #[test]
    fn all() {
        let Ok(path) = std::env::var("JUKEBOX_LOG") else { return };
        let log = std::fs::read_to_string(path).unwrap();
        for line in log.lines() {
            if let Some(rest) = line.split("game: cue ").nth(1) {
                let cue = rest.split_whitespace().nth(1).unwrap();
                println!("{cue:50} {}", pretty(cue));
            }
        }
    }
}
