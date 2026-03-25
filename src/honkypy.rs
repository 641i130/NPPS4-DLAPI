// Native Rust port of honkypy (HonokaMiku/libhonoka) for SIF database decryption.
// Translated from https://github.com/DarkEnergyProcessor/honky-py (MIT License)
//
// Supports V2 (4-byte header) and V3/V4 (16-byte header) decryption modes.
// V1 is omitted as it is not included in the probe path.

use anyhow::bail;
use md5::{Digest, Md5};

// ── Key tables (V3 per-region) ────────────────────────────────────────────────

#[rustfmt::skip]
const KEY_TABLES_JP: [u32; 64] = [
    1210253353, 1736710334, 1030507233, 1924017366, 1603299666, 1844516425, 1102797553,   32188137,
     782633907,  356258523,  957120135,   10030910,  811467044, 1226589197, 1303858438, 1423840583,
     756169139, 1304954701, 1723556931,  648430219, 1560506399, 1987934810,  305677577,  505363237,
     450129501, 1811702731, 2146795414,  842747461,  638394899,   51014537,  198914076,  120739502,
    1973027104,  586031952, 1484278592, 1560111926,  441007634, 1006001970, 2038250142,  232546121,
     827280557, 1307729428,  775964996,  483398502, 1724135019, 2125939248,  742088754, 1411519905,
     136462070, 1084053905, 2039157473, 1943671327,  650795184,  151139993, 1467120569, 1883837341,
    1249929516,  382015614, 1020618905, 1082135529,  870997426, 1221338057, 1623152467, 1020681319,
];

#[rustfmt::skip]
const KEY_TABLES_WW: [u32; 64] = [
    2861607190, 3623207331, 3775582911, 3285432773, 2211141973, 3078448744,  464780620,  714479011,
     439907422,  421011207, 2997499268,  630739911, 1488792645, 1334839443, 3136567329,  796841981,
    2604917769, 4035806207,  693592067, 1142167757, 1158290436,  568289681, 3621754479, 3645263650,
    4125133444, 3226430103, 3090611485, 1144327221,  879762852, 2932733487, 1916506591, 2754493440,
    1489123288, 3555253860, 2353824933, 1682542640,  635743937, 3455367432,  532501229, 4106615561,
    2081902950,  143042908, 2637612210, 1140910436, 3402665631,  334620177, 1874530657,  863688911,
    1651916050, 1216533340, 2730854202, 1488870464, 2778406960, 3973978011, 1602100650, 2877224961,
    1406289939, 1442089725, 2196364928, 2599396125, 2963448367, 3316646782,  322755307, 3531653795,
];

#[rustfmt::skip]
const KEY_TABLES_TW: [u32; 64] = [
    0xA925E518, 0x5AB9C4A4, 0x01950558, 0xACFF7182, 0xE8183331, 0x9D1B6963, 0x0B8E9D15, 0x96DAD0BB,
    0x0F941E35, 0xC968E363, 0x2058A6AA, 0x7176BB02, 0x4A4B2403, 0xED7A4E23, 0x3BB41EE6, 0x71634C06,
    0x7E0DD1DA, 0x343325C9, 0xE97B42F6, 0xF68F3C8F, 0x1587DED8, 0x09935F9B, 0x3273309B, 0xEFBC3178,
    0x94C01BDD, 0x40CEA3BB, 0xD5785C8A, 0x0EC1B98E, 0xC8D2D2B6, 0xEF7D77B1, 0x71814AAF, 0x2E838EAB,
    0x6B187F58, 0xA9BC924E, 0x6EAB5BA6, 0x738F6D2F, 0xC1B49AA4, 0xAB6A5D53, 0xF958F728, 0x5A0CDB5B,
    0xB8133931, 0x923336C3, 0xB5A41DE0, 0x5F819B33, 0x1F3A76AF, 0x56FB7A7C, 0x64AE7167, 0xF39C00F2,
    0x8F6F61C4, 0x6A79B9B9, 0x5B0AB1A6, 0xB7F07A0A, 0x223035FF, 0x1AA8664C, 0x553EDB16, 0x379230C6,
    0xA2AEEB8A, 0xF647D0EA, 0xA91CB2F6, 0xBB70F817, 0x94D63581, 0x49A7FAD6, 0x7BEDDD15, 0xC6913CED,
];

#[rustfmt::skip]
const KEY_TABLES_CN: [u32; 64] = [
    0x1B695658, 0x0A43A213, 0x0EAD0863, 0x1400056D, 0xD470461D, 0xB6152300, 0xFBE054BC, 0x9AC9F112,
    0x23D3CAB6, 0xCD8FE028, 0x6905BD74, 0x01A3A612, 0x6E96A579, 0x333D7AD1, 0xB6688BFF, 0x29160495,
    0xD7743BCF, 0x8EDE97BB, 0xCACB7E8D, 0x24D81C23, 0xDBFC6947, 0xB07521C8, 0xF506E2AE, 0x3F48DF2F,
    0x52BEB172, 0x695935E8, 0x13E2A0A9, 0xE2EDF409, 0x96CBA5C1, 0xDBB1E890, 0x4C2AF968, 0x17FD17C6,
    0x1B9AF5A8, 0x97C0BC25, 0x8413C879, 0xD9B13FE1, 0x4066A948, 0x9662023A, 0x74A4FEEE, 0x1F24B4F6,
    0x637688C8, 0x7A7CCF70, 0x91042EEC, 0x57EDD02C, 0x666DA2DD, 0x92839DE9, 0x43BAA9ED, 0x024A8E2C,
    0xD4EE7B72, 0x34C18B72, 0x13B275C4, 0xED506A6E, 0xBC1C29B9, 0xFA66A220, 0xC2364DE3, 0x767E52B2,
    0xE2D32439, 0xE6F0CEF5, 0xD18C8687, 0x14BBA295, 0xCD84D15B, 0xA0290F82, 0xD3E95AFC, 0x9C6A97B4,
];

// ── LCG parameters (V3/V4) ────────────────────────────────────────────────────
// Indices: 0=glibc, 1=Borland, 2=MSVC (used by V3), 3=VMS

#[derive(Copy, Clone)]
struct LcgParams {
    a: u32,
    c: u32,
    shift: u32,
}

const V4_LCG_PARAMS: [LcgParams; 4] = [
    LcgParams { a: 1103515245, c: 12345,   shift: 15 },
    LcgParams { a: 22695477,   c: 1,       shift: 23 },
    LcgParams { a: 214013,     c: 2531011, shift: 24 }, // MSVC — used by V3
    LcgParams { a: 65793,      c: 4282663, shift: 8  },
];

const PREFIXES: [&[u8]; 4] = [
    b"Hello",
    b"BFd3EnkcKa",
    b"M2o2B7i3M6o6N88",
    b"iLbs0LpvJrXm3zjdhAr4",
];

const KEY_TABLES: [&[u32; 64]; 4] = [
    &KEY_TABLES_JP,
    &KEY_TABLES_WW,
    &KEY_TABLES_TW,
    &KEY_TABLES_CN,
];

// ── Public entry point ────────────────────────────────────────────────────────

/// Decrypt a SIF database file.  `basename` must be the bare filename (no path).
/// Tries all four region prefixes with V2, then V3/V4, in order.
pub fn decrypt(basename: &str, data: &[u8]) -> anyhow::Result<Vec<u8>> {
    if data.len() < 4 {
        bail!("data too short to decrypt ({} bytes)", data.len());
    }
    let fname = basename.as_bytes();

    for i in 0..4 {
        let prefix = PREFIXES[i];
        let key_tables = KEY_TABLES[i];
        let digest = md5_digest(prefix, fname);

        // V2: 4-byte header == digest[4..8]
        if data.len() >= 4 && data[0..4] == digest[4..8] {
            return Ok(decrypt_v2(&digest, &data[4..]));
        }

        // V3/V4: first 3 header bytes == ~digest[4..7]
        if data.len() >= 16
            && data[0] == !digest[4]
            && data[1] == !digest[5]
            && data[2] == !digest[6]
        {
            return decrypt_v3v4(prefix, fname, key_tables, &digest, data);
        }
    }

    bail!("honkypy: no suitable decryption mode for {}", basename)
}

// ── MD5 helper ────────────────────────────────────────────────────────────────

fn md5_digest(prefix: &[u8], basename: &[u8]) -> [u8; 16] {
    let mut h = Md5::new();
    h.update(prefix);
    h.update(basename);
    h.finalize().into()
}

// ── V2 decryption ─────────────────────────────────────────────────────────────

/// V2 step: Park-Miller-like LCG — direct translation of the Python Schrage method.
fn v2_step(update_key: u32) -> u32 {
    let a = (update_key >> 16) as u64;
    let low = (update_key & 0xFFFF) as u64;
    let b = ((a * 0x41A70000) & 0x7FFFFFFF + low * 0x41A7) & 0xFFFFFFFF;
    let c = (a * 0x41A7) >> 15;
    let d = c + b;
    // Python: e = (d - 0x7FFFFFFF) % 2^32
    let e = d.wrapping_sub(0x7FFFFFFF) & 0xFFFFFFFF;
    if e > 0x7FFFFFFE {
        e as u32
    } else {
        (d & 0xFFFFFFFF) as u32
    }
}

fn v2_xor_key(update_key: u32) -> u16 {
    (((update_key >> 23) & 0xFF) | ((update_key >> 7) & 0xFF00)) as u16
}

fn decrypt_v2(digest: &[u8; 16], body: &[u8]) -> Vec<u8> {
    let init_key = (((digest[0] & 0x7F) as u32) << 24)
        | ((digest[1] as u32) << 16)
        | ((digest[2] as u32) << 8)
        | (digest[3] as u32);

    let mut update_key = init_key;
    let mut xor_key = v2_xor_key(init_key);
    let mut out = Vec::with_capacity(body.len());

    for (i, &byte) in body.iter().enumerate() {
        let shift = (i & 1) * 8;
        let key_byte = ((xor_key >> shift) & 0xFF) as u8;
        out.push(byte ^ key_byte);
        if i & 1 == 1 {
            update_key = v2_step(update_key);
            xor_key = v2_xor_key(update_key);
        }
    }
    out
}

// ── V3/V4 decryption ──────────────────────────────────────────────────────────

fn decrypt_v3v4(
    prefix: &[u8],
    basename: &[u8],
    key_tables: &[u32; 64],
    digest: &[u8; 16],
    data: &[u8],
) -> anyhow::Result<Vec<u8>> {
    let header = &data[..16];
    let body = &data[16..];

    let (lcg_index, init_key) = match header[7] {
        ver if ver < 2 => {
            // V3: key from key_tables indexed by name_sum
            let flip = ver == 1;
            let name_sum: u32 = prefix.iter().map(|&b| b as u32).sum::<u32>()
                + basename.iter().map(|&b| b as u32).sum::<u32>();
            let mut key = key_tables[(name_sum & 0x3F) as usize];
            if flip {
                key = !key;
            }
            (2usize, key)
        }
        2 => {
            // V4: key from md5 bytes 8-11, LCG index from header[6]
            let lcg_idx = header[6] as usize;
            if lcg_idx >= 4 {
                bail!("honkypy V4: invalid lcg_index {}", lcg_idx);
            }
            let key = ((digest[8] as u32) << 24)
                | ((digest[9] as u32) << 16)
                | ((digest[10] as u32) << 8)
                | (digest[11] as u32);
            (lcg_idx, key)
        }
        v => bail!("honkypy: unsupported version byte {}", v),
    };

    let lcg = V4_LCG_PARAMS[lcg_index];
    let mut update_key = init_key;
    let mut out = Vec::with_capacity(body.len());

    for &byte in body {
        let key_byte = ((update_key >> (lcg.shift & 0x1F)) & 0xFF) as u8;
        out.push(byte ^ key_byte);
        update_key = update_key.wrapping_mul(lcg.a).wrapping_add(lcg.c);
    }
    Ok(out)
}
