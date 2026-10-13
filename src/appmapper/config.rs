// Frozen f564288 scripts/keyd-application-mapper:65-90,546-559.
use std::{fs, io, path::Path};

pub struct Section {
    pub class: String,
    pub title: String,
    pub bindings: Vec<String>,
}

pub struct Config {
    pub sections: Vec<Section>,
}

impl Config {
    pub fn load(path: &Path) -> io::Result<Self> {
        let text = fs::read_to_string(path)?
            .replace("\r\n", "\n")
            .replace('\r', "\n");
        let mut sections: Vec<Section> = Vec::new();
        for (index, line) in text.lines().enumerate() {
            // Python str.strip also includes the four ASCII information separators.
            let line = line
                .trim_matches(|c: char| c.is_whitespace() || ('\u{1c}'..='\u{1f}').contains(&c));
            if line.starts_with('[') && line.ends_with(']') {
                let mut fields = line[1..line.len() - 1].split('|');
                sections.push(Section {
                    class: fields.next().unwrap().to_owned(),
                    title: fields.next().unwrap_or("*").to_owned(),
                    bindings: Vec::new(),
                });
            } else if !line.is_empty() && !line.starts_with('#') {
                let section = sections.last_mut().ok_or_else(|| {
                    io::Error::new(
                        io::ErrorKind::InvalidData,
                        format!(
                            "{}:{}: binding before first section",
                            path.display(),
                            index + 1
                        ),
                    )
                })?;
                section.bindings.push(line.to_owned());
            }
        }
        Ok(Self { sections })
    }

    // The caller normalizes window values, not the patterns in the configuration.
    pub fn bindings<'a>(&'a self, class: &str, title: &str) -> Vec<&'a str> {
        self.sections
            .iter()
            .filter(|section| matches(class, &section.class) && matches(title, &section.title))
            .flat_map(|section| section.bindings.iter().map(String::as_str))
            .collect()
    }
}

pub fn normalize_class(value: &str) -> String {
    normalize(value, |c| c.is_ascii_alphanumeric()).to_ascii_lowercase()
}

pub fn normalize_title(value: &str) -> String {
    normalize(value, |c| {
        let cp = c as u32;
        let index = TITLE_EXCLUSIONS.partition_point(|&(_, end)| end < cp);
        c.is_alphanumeric()
            && !TITLE_EXCLUSIONS
                .get(index)
                .is_some_and(|&(start, _)| start <= cp)
    })
    .to_lowercase()
}

fn normalize(value: &str, keep: impl Fn(char) -> bool) -> String {
    let mut result = String::new();
    let mut separator = false;
    for c in value.chars() {
        if keep(c) {
            if separator && !result.is_empty() {
                result.push('-');
            }
            result.push(c);
            separator = false;
        } else {
            separator = true;
        }
    }
    result
}

pub fn matches(value: &str, pattern: &str) -> bool {
    let pattern: Vec<char> = pattern.chars().collect();
    let value: Vec<char> = value.chars().collect();
    let (mut p, mut v) = (0, 0);
    let mut star = None;
    while v < value.len() {
        if pattern.get(p) == Some(&'*') {
            p += 1;
            star = Some((p, v));
        } else if let Some(next) = match_one(&pattern, p, value[v]) {
            p = next;
            v += 1;
        } else if let Some((after_star, retry)) = star {
            let retry = retry + 1;
            star = Some((after_star, retry));
            p = after_star;
            v = retry;
        } else {
            return false;
        }
    }
    while pattern.get(p) == Some(&'*') {
        p += 1;
    }
    p == pattern.len()
}

fn match_one(pattern: &[char], index: usize, value: char) -> Option<usize> {
    match *pattern.get(index)? {
        '?' => Some(index + 1),
        '[' => {
            let start = index + 1;
            let mut end = start;
            if pattern.get(end) == Some(&'!') {
                end += 1;
            }
            if pattern.get(end) == Some(&']') {
                end += 1;
            }
            while end < pattern.len() && pattern[end] != ']' {
                end += 1;
            }
            if end == pattern.len() {
                return (value == '[').then_some(index + 1);
            }
            match_class(&pattern[start..end], value).then_some(end + 1)
        }
        c => (c == value).then_some(index + 1),
    }
}

fn match_class(body: &[char], value: char) -> bool {
    // Python fnmatch removes descending ranges before interpreting a leading '!'.
    let mut chunks = Vec::new();
    let mut start = 0;
    let mut search = if body.first() == Some(&'!') { 2 } else { 1 };
    while search < body.len() {
        let Some(offset) = body[search..].iter().position(|&c| c == '-') else {
            break;
        };
        let dash = search + offset;
        chunks.push(body[start..dash].to_vec());
        start = dash + 1;
        search = dash + 3;
    }
    if start < body.len() {
        chunks.push(body[start..].to_vec());
    } else if let Some(last) = chunks.last_mut() {
        last.push('-');
    }
    for i in (1..chunks.len()).rev() {
        if chunks[i - 1].last() > chunks[i].first() {
            let tail = chunks.remove(i);
            chunks[i - 1].pop();
            chunks[i - 1].extend_from_slice(&tail[1..]);
        }
    }
    let mut tokens = Vec::new();
    for (i, chunk) in chunks.iter().enumerate() {
        if i != 0 {
            tokens.push(('-', false));
        }
        tokens.extend(chunk.iter().map(|&c| (c, true)));
    }
    let negated = tokens.first().is_some_and(|&(c, _)| c == '!');
    let mut i = usize::from(negated);
    let mut found = false;
    while i < tokens.len() {
        if i + 2 < tokens.len() && tokens[i + 1] == ('-', false) {
            found |= tokens[i].0 <= value && value <= tokens[i + 2].0;
            i += 3;
        } else {
            found |= tokens[i].0 == value;
            i += 1;
        }
    }
    found != negated
}

// Exhaustive pure-function capture from Python 3.14.7 / Unicode 16.0.0.
// Rust's Alphabetic property additionally includes marks and letter-like symbols,
// and Unicode 17 adds characters unknown to that Python runtime. These runs remove both.
// The retained E2E hash covers normalization of every Unicode scalar value.
const _: () = assert!(
    matches!(char::UNICODE_VERSION, (17, 0, 0)),
    "refresh mapper Unicode compatibility data before changing Rust Unicode versions"
);
const TITLE_EXCLUSIONS: &[(u32, u32)] = &[
    (0x345, 0x345),
    (0x363, 0x36F),
    (0x5B0, 0x5BD),
    (0x5BF, 0x5BF),
    (0x5C1, 0x5C2),
    (0x5C4, 0x5C5),
    (0x5C7, 0x5C7),
    (0x610, 0x61A),
    (0x64B, 0x657),
    (0x659, 0x65F),
    (0x670, 0x670),
    (0x6D6, 0x6DC),
    (0x6E1, 0x6E4),
    (0x6E7, 0x6E8),
    (0x6ED, 0x6ED),
    (0x711, 0x711),
    (0x730, 0x73F),
    (0x7A6, 0x7B0),
    (0x816, 0x817),
    (0x81B, 0x823),
    (0x825, 0x827),
    (0x829, 0x82C),
    (0x88F, 0x88F),
    (0x897, 0x897),
    (0x8D4, 0x8DF),
    (0x8E3, 0x8E9),
    (0x8F0, 0x903),
    (0x93A, 0x93B),
    (0x93E, 0x94C),
    (0x94E, 0x94F),
    (0x955, 0x957),
    (0x962, 0x963),
    (0x981, 0x983),
    (0x9BE, 0x9C4),
    (0x9C7, 0x9C8),
    (0x9CB, 0x9CC),
    (0x9D7, 0x9D7),
    (0x9E2, 0x9E3),
    (0xA01, 0xA03),
    (0xA3E, 0xA42),
    (0xA47, 0xA48),
    (0xA4B, 0xA4C),
    (0xA51, 0xA51),
    (0xA70, 0xA71),
    (0xA75, 0xA75),
    (0xA81, 0xA83),
    (0xABE, 0xAC5),
    (0xAC7, 0xAC9),
    (0xACB, 0xACC),
    (0xAE2, 0xAE3),
    (0xAFA, 0xAFC),
    (0xB01, 0xB03),
    (0xB3E, 0xB44),
    (0xB47, 0xB48),
    (0xB4B, 0xB4C),
    (0xB56, 0xB57),
    (0xB62, 0xB63),
    (0xB82, 0xB82),
    (0xBBE, 0xBC2),
    (0xBC6, 0xBC8),
    (0xBCA, 0xBCC),
    (0xBD7, 0xBD7),
    (0xC00, 0xC04),
    (0xC3E, 0xC44),
    (0xC46, 0xC48),
    (0xC4A, 0xC4C),
    (0xC55, 0xC56),
    (0xC5C, 0xC5C),
    (0xC62, 0xC63),
    (0xC81, 0xC83),
    (0xCBE, 0xCC4),
    (0xCC6, 0xCC8),
    (0xCCA, 0xCCC),
    (0xCD5, 0xCD6),
    (0xCDC, 0xCDC),
    (0xCE2, 0xCE3),
    (0xCF3, 0xCF3),
    (0xD00, 0xD03),
    (0xD3E, 0xD44),
    (0xD46, 0xD48),
    (0xD4A, 0xD4C),
    (0xD57, 0xD57),
    (0xD62, 0xD63),
    (0xD81, 0xD83),
    (0xDCF, 0xDD4),
    (0xDD6, 0xDD6),
    (0xDD8, 0xDDF),
    (0xDF2, 0xDF3),
    (0xE31, 0xE31),
    (0xE34, 0xE3A),
    (0xE4D, 0xE4D),
    (0xEB1, 0xEB1),
    (0xEB4, 0xEB9),
    (0xEBB, 0xEBC),
    (0xECD, 0xECD),
    (0xF71, 0xF83),
    (0xF8D, 0xF97),
    (0xF99, 0xFBC),
    (0x102B, 0x1036),
    (0x1038, 0x1038),
    (0x103B, 0x103E),
    (0x1056, 0x1059),
    (0x105E, 0x1060),
    (0x1062, 0x1064),
    (0x1067, 0x106D),
    (0x1071, 0x1074),
    (0x1082, 0x108D),
    (0x108F, 0x108F),
    (0x109A, 0x109D),
    (0x1712, 0x1713),
    (0x1732, 0x1733),
    (0x1752, 0x1753),
    (0x1772, 0x1773),
    (0x17B6, 0x17C8),
    (0x1885, 0x1886),
    (0x18A9, 0x18A9),
    (0x1920, 0x192B),
    (0x1930, 0x1938),
    (0x1A17, 0x1A1B),
    (0x1A55, 0x1A5E),
    (0x1A61, 0x1A74),
    (0x1ABF, 0x1AC0),
    (0x1ACC, 0x1ACE),
    (0x1B00, 0x1B04),
    (0x1B35, 0x1B43),
    (0x1B80, 0x1B82),
    (0x1BA1, 0x1BA9),
    (0x1BAC, 0x1BAD),
    (0x1BE7, 0x1BF1),
    (0x1C24, 0x1C36),
    (0x1DD3, 0x1DF4),
    (0x24B6, 0x24E9),
    (0x2DE0, 0x2DFF),
    (0xA674, 0xA67B),
    (0xA69E, 0xA69F),
    (0xA7CE, 0xA7CF),
    (0xA7D2, 0xA7D2),
    (0xA7D4, 0xA7D4),
    (0xA7F1, 0xA7F1),
    (0xA802, 0xA802),
    (0xA80B, 0xA80B),
    (0xA823, 0xA827),
    (0xA880, 0xA881),
    (0xA8B4, 0xA8C3),
    (0xA8C5, 0xA8C5),
    (0xA8FF, 0xA8FF),
    (0xA926, 0xA92A),
    (0xA947, 0xA952),
    (0xA980, 0xA983),
    (0xA9B4, 0xA9BF),
    (0xA9E5, 0xA9E5),
    (0xAA29, 0xAA36),
    (0xAA43, 0xAA43),
    (0xAA4C, 0xAA4D),
    (0xAA7B, 0xAA7D),
    (0xAAB0, 0xAAB0),
    (0xAAB2, 0xAAB4),
    (0xAAB7, 0xAAB8),
    (0xAABE, 0xAABE),
    (0xAAEB, 0xAAEF),
    (0xAAF5, 0xAAF5),
    (0xABE3, 0xABEA),
    (0xFB1E, 0xFB1E),
    (0x10376, 0x1037A),
    (0x10940, 0x10959),
    (0x10A01, 0x10A03),
    (0x10A05, 0x10A06),
    (0x10A0C, 0x10A0F),
    (0x10D24, 0x10D27),
    (0x10D69, 0x10D69),
    (0x10EAB, 0x10EAC),
    (0x10EC5, 0x10EC7),
    (0x10EFA, 0x10EFC),
    (0x11000, 0x11002),
    (0x11038, 0x11045),
    (0x11073, 0x11074),
    (0x11080, 0x11082),
    (0x110B0, 0x110B8),
    (0x110C2, 0x110C2),
    (0x11100, 0x11102),
    (0x11127, 0x11132),
    (0x11145, 0x11146),
    (0x11180, 0x11182),
    (0x111B3, 0x111BF),
    (0x111CE, 0x111CF),
    (0x1122C, 0x11234),
    (0x11237, 0x11237),
    (0x1123E, 0x1123E),
    (0x11241, 0x11241),
    (0x112DF, 0x112E8),
    (0x11300, 0x11303),
    (0x1133E, 0x11344),
    (0x11347, 0x11348),
    (0x1134B, 0x1134C),
    (0x11357, 0x11357),
    (0x11362, 0x11363),
    (0x113B8, 0x113C0),
    (0x113C2, 0x113C2),
    (0x113C5, 0x113C5),
    (0x113C7, 0x113CA),
    (0x113CC, 0x113CD),
    (0x11435, 0x11441),
    (0x11443, 0x11445),
    (0x114B0, 0x114C1),
    (0x115AF, 0x115B5),
    (0x115B8, 0x115BE),
    (0x115DC, 0x115DD),
    (0x11630, 0x1163E),
    (0x11640, 0x11640),
    (0x116AB, 0x116B5),
    (0x1171D, 0x1172A),
    (0x1182C, 0x11838),
    (0x11930, 0x11935),
    (0x11937, 0x11938),
    (0x1193B, 0x1193C),
    (0x11940, 0x11940),
    (0x11942, 0x11942),
    (0x119D1, 0x119D7),
    (0x119DA, 0x119DF),
    (0x119E4, 0x119E4),
    (0x11A01, 0x11A0A),
    (0x11A35, 0x11A39),
    (0x11A3B, 0x11A3E),
    (0x11A51, 0x11A5B),
    (0x11A8A, 0x11A97),
    (0x11B60, 0x11B67),
    (0x11C2F, 0x11C36),
    (0x11C38, 0x11C3E),
    (0x11C92, 0x11CA7),
    (0x11CA9, 0x11CB6),
    (0x11D31, 0x11D36),
    (0x11D3A, 0x11D3A),
    (0x11D3C, 0x11D3D),
    (0x11D3F, 0x11D41),
    (0x11D43, 0x11D43),
    (0x11D47, 0x11D47),
    (0x11D8A, 0x11D8E),
    (0x11D90, 0x11D91),
    (0x11D93, 0x11D96),
    (0x11DB0, 0x11DDB),
    (0x11DE0, 0x11DE9),
    (0x11EF3, 0x11EF6),
    (0x11F00, 0x11F01),
    (0x11F03, 0x11F03),
    (0x11F34, 0x11F3A),
    (0x11F3E, 0x11F40),
    (0x1611E, 0x1612E),
    (0x16EA0, 0x16EB8),
    (0x16EBB, 0x16ED3),
    (0x16F4F, 0x16F4F),
    (0x16F51, 0x16F87),
    (0x16F8F, 0x16F92),
    (0x16FF0, 0x16FF6),
    (0x187F8, 0x187FF),
    (0x18D09, 0x18D1E),
    (0x18D80, 0x18DF2),
    (0x1BC9E, 0x1BC9E),
    (0x1E000, 0x1E006),
    (0x1E008, 0x1E018),
    (0x1E01B, 0x1E021),
    (0x1E023, 0x1E024),
    (0x1E026, 0x1E02A),
    (0x1E08F, 0x1E08F),
    (0x1E6C0, 0x1E6DE),
    (0x1E6E0, 0x1E6F5),
    (0x1E6FE, 0x1E6FF),
    (0x1E947, 0x1E947),
    (0x1F130, 0x1F149),
    (0x1F150, 0x1F169),
    (0x1F170, 0x1F189),
    (0x2B73A, 0x2B73F),
    (0x2CEA2, 0x2CEAD),
    (0x323B0, 0x33479),
];
