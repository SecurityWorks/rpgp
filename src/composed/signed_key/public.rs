use std::io;

use chrono::{DateTime, Utc};
use log::warn;
use rand::{CryptoRng, Rng};

use crate::{
    armor,
    composed::{
        key::{PublicKey, PublicSubkey},
        signed_key::SignedKeyDetails,
        ArmorOptions,
    },
    crypto::{
        hash::{HashAlgorithm, KnownDigest},
        public_key::PublicKeyAlgorithm,
    },
    errors::{bail, ensure, Result},
    packet::{self, Packet, PacketTrait, SignatureType, SubpacketData},
    ser::Serialize,
    types::{
        EskType, Fingerprint, Imprint, KeyDetails, KeyId, KeyVersion, PacketLength, PkeskBytes,
        PublicKeyTrait, PublicParams, SignatureBytes, Tag,
    },
};

/// A Public OpenPGP key ("Transferable Public Key"), complete with self-signatures (and optionally
/// third party signatures). This format can be used to transfer a public key to other OpenPGP users.
///
/// An OpenPGP Transferable Public Key is also known as an OpenPGP certificate.
#[derive(Debug, PartialEq, Eq, Clone)]
pub struct SignedPublicKey {
    pub primary_key: packet::PublicKey,
    pub details: SignedKeyDetails,
    pub public_subkeys: Vec<SignedPublicSubKey>,
}

/// Parse transferable public keys from the given packets.
/// Ref: <https://www.rfc-editor.org/rfc/rfc9580.html#name-transferable-public-keys>
pub struct SignedPublicKeyParser<
    I: Sized + Iterator<Item = crate::errors::Result<crate::packet::Packet>>,
> {
    inner: std::iter::Peekable<I>,
}

impl<I: Sized + Iterator<Item = crate::errors::Result<crate::packet::Packet>>>
    SignedPublicKeyParser<I>
{
    pub fn into_inner(self) -> std::iter::Peekable<I> {
        self.inner
    }

    pub fn from_packets(packets: std::iter::Peekable<I>) -> Self {
        SignedPublicKeyParser { inner: packets }
    }
}

impl<I: Sized + Iterator<Item = Result<Packet>>> Iterator for SignedPublicKeyParser<I> {
    type Item = Result<SignedPublicKey>;

    fn next(&mut self) -> Option<Self::Item> {
        match super::key_parser::next::<_, packet::PublicKey>(
            &mut self.inner,
            Tag::PublicKey,
            false,
        ) {
            Some(Err(err)) => Some(Err(err)),
            None => None,
            Some(Ok((primary_key, details, public_subkeys, _))) => Some(Ok(SignedPublicKey::new(
                primary_key,
                details,
                public_subkeys,
            ))),
        }
    }
}

impl crate::composed::Deserializable for SignedPublicKey {
    /// Parse a transferable key from packets.
    /// Ref: <https://www.rfc-editor.org/rfc/rfc9580.html#name-transferable-public-keys>
    fn from_packets<'a, I: Iterator<Item = Result<Packet>> + 'a>(
        packets: std::iter::Peekable<I>,
    ) -> Box<dyn Iterator<Item = Result<Self>> + 'a> {
        Box::new(SignedPublicKeyParser::from_packets(packets))
    }

    fn matches_block_type(typ: armor::BlockType) -> bool {
        matches!(typ, armor::BlockType::PublicKey | armor::BlockType::File)
    }
}

impl SignedPublicKey {
    pub fn new(
        primary_key: packet::PublicKey,
        details: SignedKeyDetails,
        mut public_subkeys: Vec<SignedPublicSubKey>,
    ) -> Self {
        public_subkeys.retain(|key| {
            if key.signatures.is_empty() {
                warn!("ignoring unsigned {:?}", key.key);
                false
            } else {
                true
            }
        });

        SignedPublicKey {
            primary_key,
            details,
            public_subkeys,
        }
    }

    /// Get the public key expiration as a date.
    pub fn expires_at(&self) -> Option<DateTime<Utc>> {
        let expiration = self.details.key_expiration_time()?;
        Some(*self.primary_key.created_at() + expiration)
    }

    fn verify_public_subkeys(&self) -> Result<()> {
        for subkey in &self.public_subkeys {
            subkey.verify(&self.primary_key)?;
        }

        Ok(())
    }

    pub fn verify(&self) -> Result<()> {
        self.details.verify(&self.primary_key)?;
        self.verify_public_subkeys()?;

        Ok(())
    }

    pub fn to_armored_writer(
        &self,
        writer: &mut impl io::Write,
        opts: ArmorOptions<'_>,
    ) -> Result<()> {
        armor::write(
            self,
            armor::BlockType::PublicKey,
            writer,
            opts.headers,
            opts.include_checksum,
        )
    }

    pub fn to_armored_bytes(&self, opts: ArmorOptions<'_>) -> Result<Vec<u8>> {
        let mut buf = Vec::new();

        self.to_armored_writer(&mut buf, opts)?;

        Ok(buf)
    }

    pub fn to_armored_string(&self, opts: ArmorOptions<'_>) -> Result<String> {
        let res = String::from_utf8(self.to_armored_bytes(opts)?).map_err(|e| e.utf8_error())?;
        Ok(res)
    }

    pub fn as_unsigned(&self) -> PublicKey {
        PublicKey::new(
            self.primary_key.clone(),
            self.details.as_unsigned(),
            self.public_subkeys
                .iter()
                .map(SignedPublicSubKey::as_unsigned)
                .collect(),
        )
    }

    pub fn encrypt<R: Rng + CryptoRng>(
        &self,
        rng: R,
        plain: &[u8],
        typ: EskType,
    ) -> Result<PkeskBytes> {
        self.primary_key.encrypt(rng, plain, typ)
    }
}

impl KeyDetails for SignedPublicKey {
    fn version(&self) -> KeyVersion {
        self.primary_key.version()
    }

    fn fingerprint(&self) -> Fingerprint {
        self.primary_key.fingerprint()
    }

    fn key_id(&self) -> KeyId {
        self.primary_key.key_id()
    }

    fn algorithm(&self) -> PublicKeyAlgorithm {
        self.primary_key.algorithm()
    }
}

impl Imprint for SignedPublicKey {
    fn imprint<D: KnownDigest>(&self) -> Result<generic_array::GenericArray<u8, D::OutputSize>> {
        self.primary_key.imprint::<D>()
    }
}

impl PublicKeyTrait for SignedPublicKey {
    fn verify_signature(
        &self,
        hash: HashAlgorithm,
        data: &[u8],
        sig: &SignatureBytes,
    ) -> Result<()> {
        self.primary_key.verify_signature(hash, data, sig)
    }

    fn public_params(&self) -> &PublicParams {
        self.primary_key.public_params()
    }

    fn created_at(&self) -> &chrono::DateTime<chrono::Utc> {
        self.primary_key.created_at()
    }

    fn expiration(&self) -> Option<u16> {
        self.primary_key.expiration()
    }
}

impl Serialize for SignedPublicKey {
    fn to_writer<W: io::Write>(&self, writer: &mut W) -> Result<()> {
        self.primary_key.to_writer_with_header(writer)?;
        self.details.to_writer(writer)?;
        for ps in &self.public_subkeys {
            ps.to_writer(writer)?;
        }

        Ok(())
    }

    fn write_len(&self) -> usize {
        let key_len = self.primary_key.write_len().try_into().expect("key size");
        let mut sum = PacketLength::fixed_encoding_len(key_len);
        sum += key_len as usize;
        sum += self.details.write_len();
        sum += self.public_subkeys.write_len();
        sum
    }
}

/// Represents a Public PGP SubKey.
#[derive(Debug, PartialEq, Eq, Clone)]
pub struct SignedPublicSubKey {
    pub key: packet::PublicSubkey,
    pub signatures: Vec<packet::Signature>,
}

impl SignedPublicSubKey {
    pub fn new(key: packet::PublicSubkey, mut signatures: Vec<packet::Signature>) -> Self {
        signatures.retain(|sig| {
            if sig.typ() != Some(SignatureType::SubkeyBinding)
                && sig.typ() != Some(SignatureType::SubkeyRevocation)
            {
                warn!(
                    "ignoring unexpected signature {:?} after Subkey packet",
                    sig.typ()
                );
                false
            } else {
                true
            }
        });

        SignedPublicSubKey { key, signatures }
    }

    pub fn verify<P>(&self, key: &P) -> Result<()>
    where
        P: PublicKeyTrait + Serialize,
    {
        ensure!(!self.signatures.is_empty(), "missing subkey bindings");

        // TODO: It's sufficient if the latest binding signature is valid
        for sig in &self.signatures {
            sig.verify_subkey_binding(key, &self.key)?;

            // If the subkey is signing capable, check the embedded backward signature
            if sig.key_flags().sign() {
                let Some(backsig) = sig.embedded_signature() else {
                    bail!("missing embedded signature for signing capable subkey");
                };
                backsig.verify_primary_key_binding(&self.key, key)?;
            }
        }

        Ok(())
    }

    pub fn as_unsigned(&self) -> PublicSubkey {
        let sig = self.signatures.first().expect("missing signatures");

        let embedded = sig.config().and_then(|c| {
            c.hashed_subpackets().find_map(|p| match &p.data {
                SubpacketData::EmbeddedSignature(backsig) => Some(*backsig.clone()),
                _ => None,
            })
        });

        let keyflags = sig.key_flags();

        PublicSubkey::new(self.key.clone(), keyflags, embedded)
    }

    pub fn encrypt<R: Rng + CryptoRng>(
        &self,
        rng: R,
        plain: &[u8],
        typ: EskType,
    ) -> Result<PkeskBytes> {
        self.key.encrypt(rng, plain, typ)
    }
}

impl Imprint for SignedPublicSubKey {
    fn imprint<D: KnownDigest>(&self) -> Result<generic_array::GenericArray<u8, D::OutputSize>> {
        self.key.imprint::<D>()
    }
}

impl KeyDetails for SignedPublicSubKey {
    fn version(&self) -> KeyVersion {
        self.key.version()
    }

    /// Returns the fingerprint of the key.
    fn fingerprint(&self) -> Fingerprint {
        self.key.fingerprint()
    }
    /// Returns the Key ID of the key.
    fn key_id(&self) -> KeyId {
        self.key.key_id()
    }

    fn algorithm(&self) -> PublicKeyAlgorithm {
        self.key.algorithm()
    }
}
impl PublicKeyTrait for SignedPublicSubKey {
    fn verify_signature(
        &self,
        hash: HashAlgorithm,
        data: &[u8],
        sig: &SignatureBytes,
    ) -> Result<()> {
        self.key.verify_signature(hash, data, sig)
    }

    fn public_params(&self) -> &PublicParams {
        self.key.public_params()
    }

    fn created_at(&self) -> &chrono::DateTime<chrono::Utc> {
        self.key.created_at()
    }

    fn expiration(&self) -> Option<u16> {
        self.key.expiration()
    }
}

impl Serialize for SignedPublicSubKey {
    fn to_writer<W: io::Write>(&self, writer: &mut W) -> Result<()> {
        self.key.to_writer_with_header(writer)?;
        for sig in &self.signatures {
            sig.to_writer_with_header(writer)?;
        }

        Ok(())
    }

    fn write_len(&self) -> usize {
        let key_len = self.key.write_len().try_into().expect("key size");
        let mut sum = PacketLength::fixed_encoding_len(key_len);
        sum += key_len as usize;
        for sig in &self.signatures {
            let sig_len = sig.write_len().try_into().expect("signature size");
            sum += PacketLength::fixed_encoding_len(sig_len);
            sum += sig_len as usize;
        }
        sum
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;
    use crate::composed::shared::Deserializable;

    #[test]
    fn test_v6_annex_a_3() -> Result<()> {
        let _ = pretty_env_logger::try_init();

        // A.3. Sample v6 Certificate (Transferable Public Key)

        let c = "-----BEGIN PGP PUBLIC KEY BLOCK-----

xioGY4d/4xsAAAAg+U2nu0jWCmHlZ3BqZYfQMxmZu52JGggkLq2EVD34laPCsQYf
GwoAAABCBYJjh3/jAwsJBwUVCg4IDAIWAAKbAwIeCSIhBssYbE8GCaaX5NUt+mxy
KwwfHifBilZwj2Ul7Ce62azJBScJAgcCAAAAAK0oIBA+LX0ifsDm185Ecds2v8lw
gyU2kCcUmKfvBXbAf6rhRYWzuQOwEn7E/aLwIwRaLsdry0+VcallHhSu4RN6HWaE
QsiPlR4zxP/TP7mhfVEe7XWPxtnMUMtf15OyA51YBM4qBmOHf+MZAAAAIIaTJINn
+eUBXbki+PSAld2nhJh/LVmFsS+60WyvXkQ1wpsGGBsKAAAALAWCY4d/4wKbDCIh
BssYbE8GCaaX5NUt+mxyKwwfHifBilZwj2Ul7Ce62azJAAAAAAQBIKbpGG2dWTX8
j+VjFM21J0hqWlEg+bdiojWnKfA5AQpWUWtnNwDEM0g12vYxoWM8Y81W+bHBw805
I8kWVkXU6vFOi+HWvv/ira7ofJu16NnoUkhclkUrk0mXubZvyl4GBg==
-----END PGP PUBLIC KEY BLOCK-----";

        let (spk, _) = SignedPublicKey::from_armor_single(io::Cursor::new(c))?;

        eprintln!("spk: {spk:#02x?}");

        spk.verify()?;

        Ok(())
    }
}
