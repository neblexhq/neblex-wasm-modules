//! Standard security handler for PDFs that open without a password: owner-password
//! (permissions-only) files, common for bank statements and government forms. Covers
//! RC4 40/128-bit (R2-R4), AES-128 (R4 AESV2) and AES-256 (R5/R6). A file that needs a
//! user password to open is reported as such; nothing here tries to guess passwords.

use aes::cipher::{generic_array::GenericArray, BlockDecrypt, BlockEncrypt, KeyInit};
use aes::{Aes128, Aes256};
use md5::{Digest, Md5};
use sha2::{Sha256, Sha384, Sha512};

const PAD: [u8; 32] = [
    0x28, 0xBF, 0x4E, 0x5E, 0x4E, 0x75, 0x8A, 0x41, 0x64, 0x00, 0x4E, 0x56, 0xFF, 0xFA, 0x01, 0x08,
    0x2E, 0x2E, 0x00, 0xB6, 0xD0, 0x68, 0x3E, 0x80, 0x2F, 0x0C, 0xA9, 0xFE, 0x64, 0x53, 0x69, 0x7A,
];

#[derive(Clone, Copy, PartialEq)]
pub enum Method {
    Identity,
    Rc4,
    Aes128,
    Aes256,
}

pub struct Crypt {
    key: Vec<u8>,
    pub streams: Method,
    pub strings: Method,
    pub encrypt_metadata: bool,
}

/// What the Encrypt dictionary says, already resolved to plain values.
pub struct Params<'a> {
    pub r: u32,
    pub length_bits: u32,
    pub o: &'a [u8],
    pub u: &'a [u8],
    pub ue: &'a [u8],
    pub p: i32,
    pub id0: &'a [u8],
    pub encrypt_metadata: bool,
    pub stm: Method,
    pub str: Method,
}

impl Crypt {
    pub fn new(p: &Params) -> Result<Crypt, String> {
        let needs_password = || {
            "this PDF needs a password to open; password-protected PDFs are not supported"
                .to_string()
        };
        let key = match p.r {
            2..=4 => {
                let n = if p.r == 2 {
                    5
                } else {
                    (p.length_bits as usize / 8).clamp(5, 16)
                };
                let mut h = Md5::new();
                h.update(PAD);
                h.update(&p.o[..p.o.len().min(32)]);
                h.update(p.p.to_le_bytes());
                h.update(p.id0);
                if p.r >= 4 && !p.encrypt_metadata {
                    h.update([0xFF; 4]);
                }
                let mut k = h.finalize().to_vec();
                if p.r >= 3 {
                    for _ in 0..50 {
                        k = Md5::digest(&k[..n]).to_vec();
                    }
                }
                k.truncate(n);
                // Check the empty user password against /U before trusting the key.
                let ok = if p.r == 2 {
                    rc4(&k, &PAD) == p.u.get(..32).unwrap_or_default()
                } else {
                    let mut x = rc4(
                        &k,
                        &Md5::new().chain_update(PAD).chain_update(p.id0).finalize(),
                    );
                    for i in 1..=19u8 {
                        let ki: Vec<u8> = k.iter().map(|b| b ^ i).collect();
                        x = rc4(&ki, &x);
                    }
                    p.u.get(..16) == Some(&x[..])
                };
                if !ok {
                    return Err(needs_password());
                }
                k
            }
            5 | 6 => {
                if p.u.len() < 48 || p.ue.len() < 32 {
                    return Err("encryption dictionary is incomplete".into());
                }
                let hash = |salt: &[u8]| {
                    if p.r == 5 {
                        Sha256::digest(salt).to_vec()
                    } else {
                        hash_r6(salt)
                    }
                };
                if hash(&p.u[32..40]) != p.u[..32] {
                    return Err(needs_password());
                }
                let k = hash(&p.u[40..48]);
                let mut fk = p.ue[..32].to_vec();
                cbc_decrypt_256(&k, &[0; 16], &mut fk);
                fk
            }
            r => return Err(format!("unsupported PDF encryption (revision {r})")),
        };
        Ok(Crypt {
            key,
            streams: p.stm,
            strings: p.str,
            encrypt_metadata: p.encrypt_metadata,
        })
    }

    pub fn decrypt(&self, method: Method, num: u32, gen: u16, data: &[u8]) -> Vec<u8> {
        match method {
            Method::Identity => data.to_vec(),
            Method::Aes256 => aes_cbc(&self.key, data),
            Method::Rc4 | Method::Aes128 => {
                let mut h = Md5::new();
                h.update(&self.key);
                h.update(&num.to_le_bytes()[..3]);
                h.update(gen.to_le_bytes());
                if method == Method::Aes128 {
                    h.update(b"sAlT");
                }
                let k = h.finalize();
                let k = &k[..(self.key.len() + 5).min(16)];
                if method == Method::Rc4 {
                    rc4(k, data)
                } else {
                    aes_cbc(k, data)
                }
            }
        }
    }
}

fn rc4(key: &[u8], data: &[u8]) -> Vec<u8> {
    let mut s: [u8; 256] = core::array::from_fn(|i| i as u8);
    let mut j = 0u8;
    for i in 0..256 {
        j = j.wrapping_add(s[i]).wrapping_add(key[i % key.len()]);
        s.swap(i, j as usize);
    }
    let (mut i, mut j) = (0u8, 0u8);
    data.iter()
        .map(|&b| {
            i = i.wrapping_add(1);
            j = j.wrapping_add(s[i as usize]);
            s.swap(i as usize, j as usize);
            b ^ s[s[i as usize].wrapping_add(s[j as usize]) as usize]
        })
        .collect()
}

/// AES-CBC with the IV in the first 16 bytes and PKCS#7 padding, as PDF streams and strings use it.
fn aes_cbc(key: &[u8], data: &[u8]) -> Vec<u8> {
    if data.len() < 32 || data.len() % 16 != 0 {
        return Vec::new();
    }
    let (iv, body) = data.split_at(16);
    let mut out = body.to_vec();
    if key.len() == 32 {
        cbc_decrypt_256(key, iv, &mut out);
    } else {
        let c = Aes128::new(GenericArray::from_slice(key));
        cbc_decrypt(|b| c.decrypt_block(b), iv, &mut out);
    }
    let pad = *out.last().unwrap_or(&0) as usize;
    if (1..=16).contains(&pad) && out.len() >= pad {
        out.truncate(out.len() - pad);
    }
    out
}

fn cbc_decrypt_256(key: &[u8], iv: &[u8], data: &mut [u8]) {
    let c = Aes256::new(GenericArray::from_slice(key));
    cbc_decrypt(|b| c.decrypt_block(b), iv, data);
}

fn cbc_decrypt(
    dec: impl Fn(&mut GenericArray<u8, aes::cipher::consts::U16>),
    iv: &[u8],
    data: &mut [u8],
) {
    let mut prev: [u8; 16] = iv.try_into().unwrap_or([0; 16]);
    for chunk in data.chunks_exact_mut(16) {
        let cur: [u8; 16] = chunk.try_into().unwrap();
        let b = GenericArray::from_mut_slice(chunk);
        dec(b);
        for (x, p) in chunk.iter_mut().zip(prev) {
            *x ^= p;
        }
        prev = cur;
    }
}

/// ISO 32000-2 algorithm 2.B for the empty user password (no user key data).
fn hash_r6(salt: &[u8]) -> Vec<u8> {
    let mut k = Sha256::digest(salt).to_vec();
    let mut round = 0u32;
    loop {
        let mut e = k.repeat(64);
        let c = Aes128::new(GenericArray::from_slice(&k[..16]));
        let mut prev: [u8; 16] = k[16..32].try_into().unwrap();
        for chunk in e.chunks_exact_mut(16) {
            for (x, p) in chunk.iter_mut().zip(prev) {
                *x ^= p;
            }
            c.encrypt_block(GenericArray::from_mut_slice(chunk));
            prev = chunk.try_into().unwrap();
        }
        k = match e[..16].iter().map(|&b| b as u32).sum::<u32>() % 3 {
            0 => Sha256::digest(&e).to_vec(),
            1 => Sha384::digest(&e).to_vec(),
            _ => Sha512::digest(&e).to_vec(),
        };
        round += 1;
        if round >= 64 && (*e.last().unwrap() as u32) + 32 <= round {
            break;
        }
    }
    k.truncate(32);
    k
}
