//! Types for signatures.

use std::fmt;
use std::ops::{Deref, DerefMut};
use std::time::SystemTime;

#[cfg(any(test, feature = "quickcheck"))]
use quickcheck::{Arbitrary, Gen};

use crate::Error;
use crate::Result;
use crate::crypto::{
    mpi,
    hash::{self, Hash},
    Signer,
};
use crate::HashAlgorithm;
use crate::PublicKeyAlgorithm;
use crate::SignatureType;
use crate::packet::Signature;
use crate::packet::{
    key,
    Key,
};
use crate::packet::UserID;
use crate::packet::UserAttribute;
use crate::Packet;
use crate::packet;
use crate::packet::signature::subpacket::{
    SubpacketArea,
    SubpacketAreas,
    SubpacketTag,
};

#[cfg(any(test, feature = "quickcheck"))]
/// Like quickcheck::Arbitrary, but bounded.
trait ArbitraryBounded {
    /// Generates an arbitrary value, but only recurses if `depth >
    /// 0`.
    fn arbitrary_bounded<G: Gen>(g: &mut G, depth: usize) -> Self;
}

#[cfg(any(test, feature = "quickcheck"))]
/// Default depth when implementing Arbitrary using ArbitraryBounded.
const DEFAULT_ARBITRARY_DEPTH: usize = 2;

#[cfg(any(test, feature = "quickcheck"))]
macro_rules! impl_arbitrary_with_bound {
    ($typ:path) => {
        impl Arbitrary for $typ {
            fn arbitrary<G: Gen>(g: &mut G) -> Self {
                Self::arbitrary_bounded(
                    g,
                    crate::packet::signature::DEFAULT_ARBITRARY_DEPTH)
            }
        }
    }
}

pub mod subpacket;

/// The data stored in a `Signature` packet.
///
/// This data structure contains exactly those fields that appear in a
/// `Signature` packet.  It is used by both `Signature4` and
/// `SignatureBuilder`, which include auxiliary information.  This
/// data structure is public so that `Signature4` and
/// `SignatureBuilder` can deref to it.
#[derive(Clone, Hash, PartialEq, Eq)]
pub struct SignatureFields {
    /// Version of the signature packet. Must be 4.
    version: u8,
    /// Type of signature.
    typ: SignatureType,
    /// Public-key algorithm used for this signature.
    pk_algo: PublicKeyAlgorithm,
    /// Hash algorithm used to compute the signature.
    hash_algo: HashAlgorithm,
    /// Subpackets.
    subpackets: SubpacketAreas,
}

#[cfg(any(test, feature = "quickcheck"))]
impl ArbitraryBounded for SignatureFields {
    fn arbitrary_bounded<G: Gen>(g: &mut G, depth: usize) -> Self {
        SignatureFields {
            // XXX: Make this more interesting once we dig other
            // versions.
            version: 4,
            typ: Arbitrary::arbitrary(g),
            pk_algo: PublicKeyAlgorithm::arbitrary_for_signing(g),
            hash_algo: Arbitrary::arbitrary(g),
            subpackets: ArbitraryBounded::arbitrary_bounded(g, depth),
        }
    }
}

#[cfg(any(test, feature = "quickcheck"))]
impl_arbitrary_with_bound!(SignatureFields);

impl Deref for SignatureFields {
    type Target = SubpacketAreas;

    fn deref(&self) -> &Self::Target {
        &self.subpackets
    }
}

impl DerefMut for SignatureFields {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.subpackets
    }
}

impl SignatureFields {
    /// Gets the version.
    pub fn version(&self) -> u8 {
        self.version
    }

    /// Gets the signature type.
    pub fn typ(&self) -> SignatureType {
        self.typ
    }

    /// Gets the public key algorithm.
    ///
    /// This is `pub(crate)`, because it shouldn't be exported by
    /// `SignatureBuilder` where it is only set at the end.
    pub(crate) fn pk_algo(&self) -> PublicKeyAlgorithm {
        self.pk_algo
    }

    /// Gets the hash algorithm.
    pub fn hash_algo(&self) -> HashAlgorithm {
        self.hash_algo
    }
}

/// Builds a signature packet.
///
/// This is the mutable version of a `Signature4` packet.  To convert
/// it to one, use [`sign_hash`], [`sign_message`],
/// [`sign_direct_key`], [`sign_subkey_binding`],
/// [`sign_primary_key_binding`], [`sign_userid_binding`],
/// [`sign_user_attribute_binding`], [`sign_standalone`], or
/// [`sign_timestamp`],
///
///   [`sign_hash`]: #method.sign_hash
///   [`sign_message`]: #method.sign_message
///   [`sign_direct_key`]: #method.sign_direct_key
///   [`sign_subkey_binding`]: #method.sign_subkey_binding
///   [`sign_primary_key_binding`]: #method.sign_primary_key_binding
///   [`sign_userid_binding`]: #method.sign_userid_binding
///   [`sign_user_attribute_binding`]: #method.sign_user_attribute_binding
///   [`sign_standalone`]: #method.sign_standalone
///   [`sign_timestamp`]: #method.sign_timestamp
///
/// When finalizing the `SignatureBuilder`, an [`Issuer`] subpacket
/// and an [`IssuerFingerprint`] subpacket referencing the signing key
/// are added to the unhashed subpacket area if neither an [`Issuer`]
/// subpacket nor an [`IssuerFingerprint`] subpacket is present in
/// either of the subpacket areas.  Note: when converting a
/// `Signature` to a `SignatureBuilder`, any [`Issuer`] subpackets or
/// [`IssuerFingerprint`] subpackets are removed.  Caution: using the
/// wrong issuer, or not including an issuer at all will make the
/// signature unverifiable by most OpenPGP implementations.
///
///   [`Issuer`]: https://tools.ietf.org/html/rfc4880#section-5.2.3.5
///   [`IssuerFingerprint`]: https://www.ietf.org/id/draft-ietf-openpgp-rfc4880bis-09.html#section-5.2.3.28
///   [`set_issuer`]: #method.set_issuer
///   [`set_issuer_fingerprint`]: #method.set_issuer_fingerprint
///
/// According to [Section 5.2.3.4 of RFC 4880], `Signatures` must
/// include a `Signature Creation Time` subpacket.  When finalizing a
/// `SignatureBuilder`, we automatically insert a creation time
/// subpacket with the current time into the hashed subpacket area.
/// To override this behavior, use [`set_signature_creation_time`].
/// Note: when converting an existing `Signature` into a
/// `SignatureBuilder`, any existing `Signature Creation Time`
/// subpackets are removed.
///
///   [Section 5.2.3.4 of RFC 4880]: https://tools.ietf.org/html/rfc4880#section-5.2.3.4
///   [`set_signature_creation_time`]: #method.set_signature_creation_time
///
// IMPORTANT: If you add fields to this struct, you need to explicitly
// IMPORTANT: implement PartialEq, Eq, and Hash.
#[derive(Clone, Hash, PartialEq, Eq)]
pub struct SignatureBuilder {
    overrode_creation_time: bool,
    original_creation_time: Option<SystemTime>,
    fields: SignatureFields,
}

impl Deref for SignatureBuilder {
    type Target = SignatureFields;

    fn deref(&self) -> &Self::Target {
        &self.fields
    }
}

impl DerefMut for SignatureBuilder {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.fields
    }
}

impl SignatureBuilder {
    /// Returns a new `SignatureBuilder` object.
    pub fn new(typ: SignatureType) ->  Self {
        SignatureBuilder {
            overrode_creation_time: false,
            original_creation_time: None,
            fields: SignatureFields {
                version: 4,
                typ,
                pk_algo: PublicKeyAlgorithm::Unknown(0),
                hash_algo: HashAlgorithm::default(),
                subpackets: SubpacketAreas::default(),
            }
        }
    }

    /// Sets the signature type.
    pub fn set_type(mut self, t: SignatureType) -> Self {
        self.typ = t;
        self
    }

    /// Sets the hash algorithm.
    pub fn set_hash_algo(mut self, h: HashAlgorithm) -> Self {
        self.hash_algo = h;
        self
    }

    /// Generates a standalone signature.
    ///
    /// A [Standalone Signature] ([`SignatureType::Standalone`]) is a
    /// self-contained signature, which is only over the signature
    /// packet.
    ///
    ///   [Standalone Signature]: https://tools.ietf.org/html/rfc4880#section-5.2.1
    ///   [`SignatureType::Standalone`]: ../../types/enum.SignatureType.html#variant.Standalone
    ///
    /// This function checks that the [signature type] (passed to
    /// [`SignatureBuilder::new`], set via
    /// [`SignatureBuilder::set_type`], or copied when using
    /// `SignatureBuilder::From`) is [`SignatureType::Standalone`] or
    /// [`SignatureType::Unknown`].
    ///
    ///   [signature type]: ../../types/enum.SignatureType.html
    ///   [`SignatureBuilder::new`]: #method.new
    ///   [`SignatureBuilder::set_type`]: #method.set_type
    ///   [`SignatureType::Timestamp`]: ../../types/enum.SignatureType.html#variant.Timestamp
    ///   [`SignatureType::Unknown`]: ../../types/enum.SignatureType.html#variant.Unknown
    ///
    /// The [`Signature`]'s public-key algorithm field is set to the
    /// algorithm used by `signer`.
    ///
    ///   [`Signature`]: ../enum.Signature.html
    ///
    /// If neither an [`Issuer`] subpacket (set using
    /// [`SignatureBuilder::set_issuer`], for instance) nor an
    /// [`Issuer Fingerprint`] subpacket (set using
    /// [`SignatureBuilder::set_issuer_fingerprint`], for instance) is
    /// set, they are both added to the new `Signature`'s unhashed
    /// subpacket area and set to the `signer`'s `KeyID` and
    /// `Fingerprint`, respectively.
    ///
    ///   [`Issuer`]: https://tools.ietf.org/html/rfc4880#section-5.2.3.5
    ///   [`SignatureBuilder::set_issuer`]: #method.set_issuer
    ///   [`Issuer Fingerprint`]: https://www.ietf.org/id/draft-ietf-openpgp-rfc4880bis-09.html#section-5.2.3.28
    ///   [`SignatureBuilder::set_issuer_fingerprint`]: #method.set_issuer_fingerprint
    ///
    /// Likewise, a [`Signature Creation Time`] subpacket set to the
    /// current time is added to the hashed area if the `Signature
    /// Creation Time` subpacket hasn't been set using, for instance,
    /// the [`set_signature_creation_time`] method or the
    /// [`preserve_signature_creation_time`] method.
    ///
    ///   [`Signature Creation Time`]: https://tools.ietf.org/html/rfc4880#section-5.2.3.4
    ///   [`set_signature_creation_time`]: #method.set_signature_creation_time
    ///   [`preserve_signature_creation_time`]: #method.preserve_signature_creation_time
    ///
    /// # Examples
    ///
    /// ```
    /// use sequoia_openpgp as openpgp;
    /// use openpgp::cert::prelude::*;
    /// use openpgp::packet::prelude::*;
    /// use openpgp::policy::StandardPolicy;
    /// use openpgp::types::SignatureType;
    ///
    /// # fn main() -> openpgp::Result<()> {
    /// let p = &StandardPolicy::new();
    ///
    /// let (cert, _) = CertBuilder::new().add_signing_subkey().generate()?;
    ///
    /// // Get a usable (alive, non-revoked) signing key.
    /// let key : &Key<_, _> = cert
    ///     .keys().with_policy(p, None)
    ///     .for_signing().alive().revoked(false).nth(0).unwrap().key();
    /// // Derive a signer.
    /// let mut signer = key.clone().parts_into_secret()?.into_keypair()?;
    ///
    /// let sig = SignatureBuilder::new(SignatureType::Standalone)
    ///     .sign_standalone(&mut signer)?;
    ///
    /// // Verify it.
    /// sig.verify_standalone(signer.public())?;
    /// # Ok(())
    /// # }
    /// ```
    pub fn sign_standalone(mut self, signer: &mut dyn Signer)
                           -> Result<Signature>
    {
        match self.typ {
            SignatureType::Standalone => (),
            SignatureType::Unknown(_) => (),
            _ => return Err(Error::UnsupportedSignatureType(self.typ).into()),
        }

        self = self.pre_sign(signer)?;

        let digest = Signature::hash_standalone(&self)?;

        self.sign(signer, digest)
    }

    /// Generates a Timestamp Signature.
    ///
    /// Like a [Standalone Signature] (created using
    /// [`SignatureBuilder::sign_standalone`]), a [Timestamp
    /// Signature] is a self-contained signature, but its emphasis in
    /// on the contained timestamp, specifically, the timestamp stored
    /// in the [`Signature Creation Time`] subpacket.  This type of
    /// signature is primarily used by [timestamping services].  To
    /// timestamp a signature, you can include either a [Signature
    /// Target subpacket] (set using
    /// [`SignatureBuilder::set_signature_target`]), or an [Embedded
    /// Signature] (set using
    /// [`SignatureBuilder::set_embedded_signature`]) in the hashed
    /// area.
    ///
    ///
    ///   [Standalone Signature]: https://tools.ietf.org/html/rfc4880#section-5.2.1
    ///   [`SignatureBuilder::sign_standalone`]: #method.sign_standalone
    ///   [Timestamp Signature]: https://tools.ietf.org/html/rfc4880#section-5.2.1
    ///   [`Signature Creation Time`]: https://tools.ietf.org/html/rfc4880#section-5.2.3.4
    ///   [timestamping services]: https://en.wikipedia.org/wiki/Trusted_timestamping
    ///   [Signature Target subpacket]: https://tools.ietf.org/html/rfc4880#section-5.2.3.25
    ///   [`SignatureBuilder::set_signature_target`]: #method.set_signature_target
    ///   [Embedded Signature]: https://tools.ietf.org/html/rfc4880#section-5.2.3.26
    ///   [`SignatureBuilder::set_embedded_signature`]: #method.set_embedded_signature
    ///
    /// This function checks that the [signature type] (passed to
    /// [`SignatureBuilder::new`], set via
    /// [`SignatureBuilder::set_type`], or copied when using
    /// `SignatureBuilder::From`) is [`SignatureType::Timestamp`] or
    /// [`SignatureType::Unknown`].
    ///
    ///   [signature type]: ../../types/enum.SignatureType.html
    ///   [`SignatureBuilder::new`]: #method.new
    ///   [`SignatureBuilder::set_type`]: #method.set_type
    ///   [`SignatureType::Timestamp`]: ../../types/enum.SignatureType.html#variant.Timestamp
    ///   [`SignatureType::Unknown`]: ../../types/enum.SignatureType.html#variant.Unknown
    ///
    /// The [`Signature`]'s public-key algorithm field is set to the
    /// algorithm used by `signer`.
    ///
    ///   [`Signature`]: ../enum.Signature.html
    ///
    /// If neither an [`Issuer`] subpacket (set using
    /// [`SignatureBuilder::set_issuer`], for instance) nor an
    /// [`Issuer Fingerprint`] subpacket (set using
    /// [`SignatureBuilder::set_issuer_fingerprint`], for instance) is
    /// set, they are both added to the new `Signature`'s unhashed
    /// subpacket area and set to the `signer`'s `KeyID` and
    /// `Fingerprint`, respectively.
    ///
    ///   [`Issuer`]: https://tools.ietf.org/html/rfc4880#section-5.2.3.5
    ///   [`SignatureBuilder::set_issuer`]: #method.set_issuer
    ///   [`Issuer Fingerprint`]: https://www.ietf.org/id/draft-ietf-openpgp-rfc4880bis-09.html#section-5.2.3.28
    ///   [`SignatureBuilder::set_issuer_fingerprint`]: #method.set_issuer_fingerprint
    ///
    /// Likewise, a [`Signature Creation Time`] subpacket set to the
    /// current time is added to the hashed area if the `Signature
    /// Creation Time` subpacket hasn't been set using, for instance,
    /// the [`set_signature_creation_time`] method or the
    /// [`preserve_signature_creation_time`] method.
    ///
    ///   [`Signature Creation Time`]: https://tools.ietf.org/html/rfc4880#section-5.2.3.4
    ///   [`set_signature_creation_time`]: #method.set_signature_creation_time
    ///   [`preserve_signature_creation_time`]: #method.preserve_signature_creation_time
    ///
    /// # Examples
    ///
    /// Create a timestamp signature:
    ///
    /// ```
    /// use sequoia_openpgp as openpgp;
    /// use openpgp::cert::prelude::*;
    /// use openpgp::packet::prelude::*;
    /// use openpgp::policy::StandardPolicy;
    /// use openpgp::types::SignatureType;
    ///
    /// # fn main() -> openpgp::Result<()> {
    /// let p = &StandardPolicy::new();
    ///
    /// let (cert, _) = CertBuilder::new().add_signing_subkey().generate()?;
    ///
    /// // Get a usable (alive, non-revoked) signing key.
    /// let key : &Key<_, _> = cert
    ///     .keys().with_policy(p, None)
    ///     .for_signing().alive().revoked(false).nth(0).unwrap().key();
    /// // Derive a signer.
    /// let mut signer = key.clone().parts_into_secret()?.into_keypair()?;
    ///
    /// let sig = SignatureBuilder::new(SignatureType::Timestamp)
    ///     .sign_timestamp(&mut signer)?;
    ///
    /// // Verify it.
    /// sig.verify_timestamp(signer.public())?;
    /// # Ok(())
    /// # }
    /// ```
    pub fn sign_timestamp(mut self, signer: &mut dyn Signer)
                          -> Result<Signature>
    {
        match self.typ {
            SignatureType::Timestamp => (),
            SignatureType::Unknown(_) => (),
            _ => return Err(Error::UnsupportedSignatureType(self.typ).into()),
        }

        self = self.pre_sign(signer)?;

        let digest = Signature::hash_timestamp(&self)?;

        self.sign(signer, digest)
    }

    /// Generates a Direct Key Signature.
    ///
    /// A [Direct Key Signature] is a signature over the primary key.
    /// It is primarily used to hold fallback [preferences].  For
    /// instance, when addressing the Certificate by a User ID, the
    /// OpenPGP implementation is supposed to look for preferences
    /// like the [Preferred Symmetric Algorithms] on the User ID, and
    /// only if there is no such packet, look on the direct key
    /// signature.
    ///
    /// This function is also used to create a [Key Revocation
    /// Signature], which revokes the certificate.
    ///
    ///   [preferences]: ../../cert/trait.Preferences.html
    ///   [Direct Key Signature]: https://tools.ietf.org/html/rfc4880#section-5.2.1
    ///   [Preferred Symmetric Algorithms]: https://tools.ietf.org/html/rfc4880#section-5.2.3.7
    ///   [Key Revocation Signature]: https://tools.ietf.org/html/rfc4880#section-5.2.1
    ///
    /// This function checks that the [signature type] (passed to
    /// [`SignatureBuilder::new`], set via
    /// [`SignatureBuilder::set_type`], or copied when using
    /// `SignatureBuilder::From`) is [`SignatureType::DirectKey`],
    /// [`SignatureType::KeyRevocation`], or
    /// [`SignatureType::Unknown`].
    ///
    ///   [signature type]: ../../types/enum.SignatureType.html
    ///   [`SignatureBuilder::new`]: #method.new
    ///   [`SignatureBuilder::set_type`]: #method.set_type
    ///   [`SignatureType::DirectKey`]: ../../types/enum.SignatureType.html#variant.DirectKey
    ///   [`SignatureType::KeyRevocation`]: ../../types/enum.SignatureType.html#variant.KeyRevocation
    ///   [`SignatureType::Unknown`]: ../../types/enum.SignatureType.html#variant.Unknown
    ///
    /// The [`Signature`]'s public-key algorithm field is set to the
    /// algorithm used by `signer`.
    ///
    ///   [`Signature`]: ../enum.Signature.html
    ///
    /// If neither an [`Issuer`] subpacket (set using
    /// [`SignatureBuilder::set_issuer`], for instance) nor an
    /// [`Issuer Fingerprint`] subpacket (set using
    /// [`SignatureBuilder::set_issuer_fingerprint`], for instance) is
    /// set, they are both added to the new `Signature`'s unhashed
    /// subpacket area and set to the `signer`'s `KeyID` and
    /// `Fingerprint`, respectively.
    ///
    ///   [`Issuer`]: https://tools.ietf.org/html/rfc4880#section-5.2.3.5
    ///   [`SignatureBuilder::set_issuer`]: #method.set_issuer
    ///   [`Issuer Fingerprint`]: https://www.ietf.org/id/draft-ietf-openpgp-rfc4880bis-09.html#section-5.2.3.28
    ///   [`SignatureBuilder::set_issuer_fingerprint`]: #method.set_issuer_fingerprint
    ///
    /// Likewise, a [`Signature Creation Time`] subpacket set to the
    /// current time is added to the hashed area if the `Signature
    /// Creation Time` subpacket hasn't been set using, for instance,
    /// the [`set_signature_creation_time`] method or the
    /// [`preserve_signature_creation_time`] method.
    ///
    ///   [`Signature Creation Time`]: https://tools.ietf.org/html/rfc4880#section-5.2.3.4
    ///   [`set_signature_creation_time`]: #method.set_signature_creation_time
    ///   [`preserve_signature_creation_time`]: #method.preserve_signature_creation_time
    ///
    /// # Examples
    ///
    /// Set the default value for the [Preferred Symmetric Algorithms
    /// subpacket]:
    ///
    /// [Preferred Symmetric Algorithms subpacket]: #method.set_preferred_symmetric_algorithms
    ///
    /// ```
    /// use sequoia_openpgp as openpgp;
    /// use openpgp::cert::prelude::*;
    /// use openpgp::packet::prelude::*;
    /// use openpgp::policy::StandardPolicy;
    /// use openpgp::types::SignatureType;
    /// use openpgp::types::SymmetricAlgorithm;
    ///
    /// # fn main() -> openpgp::Result<()> {
    /// let p = &StandardPolicy::new();
    ///
    /// let (cert, _) = CertBuilder::new().add_signing_subkey().generate()?;
    ///
    /// // Get a usable (alive, non-revoked) certification key.
    /// let key : &Key<_, _> = cert
    ///     .keys().with_policy(p, None)
    ///     .for_certification().alive().revoked(false).nth(0).unwrap().key();
    /// // Derive a signer.
    /// let mut signer = key.clone().parts_into_secret()?.into_keypair()?;
    ///
    /// // A direct key signature is always over the primary key.
    /// let pk = cert.primary_key().key();
    ///
    /// // Modify the existing direct key signature.
    /// let sig = SignatureBuilder::from(
    ///         cert.with_policy(p, None)?.direct_key_signature()?.clone())
    ///     .set_preferred_symmetric_algorithms(
    ///         vec![ SymmetricAlgorithm::AES256,
    ///               SymmetricAlgorithm::AES128,
    ///         ])?
    ///     .sign_direct_key(&mut signer, pk)?;
    ///
    /// // Verify it.
    /// sig.verify_direct_key(signer.public(), pk)?;
    /// # Ok(())
    /// # }
    /// ```
    pub fn sign_direct_key<P>(mut self, signer: &mut dyn Signer,
                              pk: &Key<P, key::PrimaryRole>)
        -> Result<Signature>
        where P: key::KeyParts,
    {
        match self.typ {
            SignatureType::DirectKey => (),
            SignatureType::KeyRevocation => (),
            SignatureType::Unknown(_) => (),
            _ => return Err(Error::UnsupportedSignatureType(self.typ).into()),
        }

        self = self.pre_sign(signer)?;

        let digest = Signature::hash_direct_key(&self, pk)?;

        self.sign(signer, digest)
    }

    /// Generates a User ID binding signature.
    ///
    /// A User ID binding signature (a self signature) or a [User ID
    /// certification] (a third-party signature) is a signature over a
    /// `User ID` and a `Primary Key` made by a certification-capable
    /// key.  It asserts that the signer is convinced that the `User
    /// ID` should be associated with the `Certificate`, i.e., that
    /// the binding is authentic.
    ///
    ///   [User ID certification]: https://tools.ietf.org/html/rfc4880#section-5.2.1
    ///
    /// OpenPGP has four types of `User ID` certifications.  They are
    /// intended to express the degree of the signer's conviction,
    /// i.e., how well the signer authenticated the binding.  In
    /// practice, the `Positive Certification` type is used for
    /// self-signatures, and the `Generic Certification` is used for
    /// third-party certifications; the other types are not normally
    /// used.
    ///
    /// This function is also used to create [Certification
    /// Revocations].
    ///
    ///   [Certification Revocations]: https://tools.ietf.org/html/rfc4880#section-5.2.1
    ///
    /// This function checks that the [signature type] (passed to
    /// [`SignatureBuilder::new`], set via
    /// [`SignatureBuilder::set_type`], or copied when using
    /// `SignatureBuilder::From`) is [`GenericCertification`],
    /// [`PersonaCertification`], [`CasualCertification`],
    /// [`PositiveCertification`], [`CertificationRevocation`], or
    /// [`SignatureType::Unknown`].
    ///
    ///   [signature type]: ../../types/enum.SignatureType.html
    ///   [`SignatureBuilder::new`]: #method.new
    ///   [`SignatureBuilder::set_type`]: #method.set_type
    ///   [`GenericCertification`]: ../../types/enum.SignatureType.html#variant.GenericCertification
    ///   [`PersonaCertification`]: ../../types/enum.SignatureType.html#variant.PersonaCertification
    ///   [`CasualCertification`]: ../../types/enum.SignatureType.html#variant.CasualCertification
    ///   [`PositiveCertification`]: ../../types/enum.SignatureType.html#variant.PositiveCertification
    ///   [`CertificationRevocation`]: ../../types/enum.SignatureType.html#variant.CertificationRevocation
    ///   [`SignatureType::Unknown`]: ../../types/enum.SignatureType.html#variant.Unknown
    ///
    /// The [`Signature`]'s public-key algorithm field is set to the
    /// algorithm used by `signer`.
    ///
    ///   [`Signature`]: ../enum.Signature.html
    ///
    /// If neither an [`Issuer`] subpacket (set using
    /// [`SignatureBuilder::set_issuer`], for instance) nor an
    /// [`Issuer Fingerprint`] subpacket (set using
    /// [`SignatureBuilder::set_issuer_fingerprint`], for instance) is
    /// set, they are both added to the new `Signature`'s unhashed
    /// subpacket area and set to the `signer`'s `KeyID` and
    /// `Fingerprint`, respectively.
    ///
    ///   [`Issuer`]: https://tools.ietf.org/html/rfc4880#section-5.2.3.5
    ///   [`SignatureBuilder::set_issuer`]: #method.set_issuer
    ///   [`Issuer Fingerprint`]: https://www.ietf.org/id/draft-ietf-openpgp-rfc4880bis-09.html#section-5.2.3.28
    ///   [`SignatureBuilder::set_issuer_fingerprint`]: #method.set_issuer_fingerprint
    ///
    /// Likewise, a [`Signature Creation Time`] subpacket set to the
    /// current time is added to the hashed area if the `Signature
    /// Creation Time` subpacket hasn't been set using, for instance,
    /// the [`set_signature_creation_time`] method or the
    /// [`preserve_signature_creation_time`] method.
    ///
    ///   [`Signature Creation Time`]: https://tools.ietf.org/html/rfc4880#section-5.2.3.4
    ///   [`set_signature_creation_time`]: #method.set_signature_creation_time
    ///   [`preserve_signature_creation_time`]: #method.preserve_signature_creation_time
    ///
    /// # Examples
    ///
    /// Set the [Preferred Symmetric Algorithms subpacket], which will
    /// be used when addressing the certificate via the associated
    /// User ID:
    ///
    /// [Preferred Symmetric Algorithms subpacket]: #method.set_preferred_symmetric_algorithms
    ///
    /// ```
    /// use sequoia_openpgp as openpgp;
    /// use openpgp::cert::prelude::*;
    /// use openpgp::packet::prelude::*;
    /// use openpgp::policy::StandardPolicy;
    /// use openpgp::types::SymmetricAlgorithm;
    ///
    /// # fn main() -> openpgp::Result<()> {
    /// let p = &StandardPolicy::new();
    ///
    /// let (cert, _) = CertBuilder::new().add_userid("Alice").generate()?;
    ///
    /// // Get a usable (alive, non-revoked) certification key.
    /// let key : &Key<_, _> = cert
    ///     .keys().with_policy(p, None)
    ///     .for_certification().alive().revoked(false).nth(0).unwrap().key();
    /// // Derive a signer.
    /// let mut signer = key.clone().parts_into_secret()?.into_keypair()?;
    ///
    /// let pk = cert.primary_key().key();
    ///
    /// // Update the User ID's binding signature.
    /// let ua = cert.with_policy(p, None)?.userids().nth(0).unwrap();
    /// let new_sig = SignatureBuilder::from(
    ///         ua.binding_signature().clone())
    ///     .set_preferred_symmetric_algorithms(
    ///         vec![ SymmetricAlgorithm::AES256,
    ///               SymmetricAlgorithm::AES128,
    ///         ])?
    ///     .sign_userid_binding(&mut signer, pk, ua.userid())?;
    ///
    /// // Verify it.
    /// new_sig.verify_userid_binding(signer.public(), pk, ua.userid())?;
    /// # Ok(())
    /// # }
    /// ```
    pub fn sign_userid_binding<P>(mut self, signer: &mut dyn Signer,
                                  key: &Key<P, key::PrimaryRole>,
                                  userid: &UserID)
        -> Result<Signature>
        where P: key::KeyParts,
    {
        match self.typ {
            SignatureType::GenericCertification => (),
            SignatureType::PersonaCertification => (),
            SignatureType::CasualCertification => (),
            SignatureType::PositiveCertification => (),
            SignatureType::CertificationRevocation => (),
            SignatureType::Unknown(_) => (),
            _ => return Err(Error::UnsupportedSignatureType(self.typ).into()),
        }

        self = self.pre_sign(signer)?;

        let digest = Signature::hash_userid_binding(&self, key, userid)?;

        self.sign(signer, digest)
    }

    /// Generates a subkey binding signature.
    ///
    /// A [subkey binding signature] is a signature over the primary
    /// key and a subkey, which is made by the primary key.  It is an
    /// assertion by the certificate that the subkey really belongs to
    /// the certificate.  That is, it binds the subkey to the
    /// certificate.
    ///
    /// Note: this function does not create a back signature, which is
    /// needed by certification-capable, signing-capable, and
    /// authentication-capable subkeys.  A back signature can be
    /// created using [`SignatureBuilder::sign_primary_key_binding`].
    ///
    /// This function is also used to create subkey revocations.
    ///
    ///   [subkey binding signature]: https://tools.ietf.org/html/rfc4880#section-5.2.1
    ///   [`SignatureBuilder::sign_primary_key_binding`]: #method.sign_primary_key_binding
    ///
    /// This function checks that the [signature type] (passed to
    /// [`SignatureBuilder::new`], set via
    /// [`SignatureBuilder::set_type`], or copied when using
    /// `SignatureBuilder::From`) is
    /// [`SignatureType::SubkeyBinding`], [`SignatureType::SubkeyRevocation`], or
    /// [`SignatureType::Unknown`].
    ///
    ///   [signature type]: ../../types/enum.SignatureType.html
    ///   [`SignatureBuilder::new`]: #method.new
    ///   [`SignatureBuilder::set_type`]: #method.set_type
    ///   [`SignatureType::SubkeyBinding`]: ../../types/enum.SignatureType.html#variant.SubkeyBinding
    ///   [`SignatureType::SubkeyRevocation`]: ../../types/enum.SignatureType.html#variant.SubkeyRevocation
    ///   [`SignatureType::Unknown`]: ../../types/enum.SignatureType.html#variant.Unknown
    ///
    /// The [`Signature`]'s public-key algorithm field is set to the
    /// algorithm used by `signer`.
    ///
    ///   [`Signature`]: ../enum.Signature.html
    ///
    /// If neither an [`Issuer`] subpacket (set using
    /// [`SignatureBuilder::set_issuer`], for instance) nor an
    /// [`Issuer Fingerprint`] subpacket (set using
    /// [`SignatureBuilder::set_issuer_fingerprint`], for instance) is
    /// set, they are both added to the new `Signature`'s unhashed
    /// subpacket area and set to the `signer`'s `KeyID` and
    /// `Fingerprint`, respectively.
    ///
    ///   [`Issuer`]: https://tools.ietf.org/html/rfc4880#section-5.2.3.5
    ///   [`SignatureBuilder::set_issuer`]: #method.set_issuer
    ///   [`Issuer Fingerprint`]: https://www.ietf.org/id/draft-ietf-openpgp-rfc4880bis-09.html#section-5.2.3.28
    ///   [`SignatureBuilder::set_issuer_fingerprint`]: #method.set_issuer_fingerprint
    ///
    /// Likewise, a [`Signature Creation Time`] subpacket set to the
    /// current time is added to the hashed area if the `Signature
    /// Creation Time` subpacket hasn't been set using, for instance,
    /// the [`set_signature_creation_time`] method or the
    /// [`preserve_signature_creation_time`] method.
    ///
    ///   [`Signature Creation Time`]: https://tools.ietf.org/html/rfc4880#section-5.2.3.4
    ///   [`set_signature_creation_time`]: #method.set_signature_creation_time
    ///   [`preserve_signature_creation_time`]: #method.preserve_signature_creation_time
    ///
    /// # Examples
    ///
    /// Add a new subkey intended for encrypting data in motion to an
    /// existing certificate:
    ///
    /// ```
    /// use sequoia_openpgp as openpgp;
    /// use openpgp::cert::prelude::*;
    /// use openpgp::packet::prelude::*;
    /// use openpgp::policy::StandardPolicy;
    /// use openpgp::types::KeyFlags;
    /// use openpgp::types::SignatureType;
    ///
    /// # fn main() -> openpgp::Result<()> {
    /// let p = &StandardPolicy::new();
    ///
    /// let (cert, _) = CertBuilder::new().generate()?;
    /// # assert_eq!(cert.keys().count(), 1);
    ///
    /// let pk = cert.primary_key().key().clone().parts_into_secret()?;
    /// // Derive a signer.
    /// let mut pk_signer = pk.clone().into_keypair()?;
    ///
    /// // Generate an encryption subkey.
    /// let mut subkey: Key<_, _> = Key4::generate_rsa(3072)?.into();
    /// // Derive a signer.
    /// let mut sk_signer = subkey.clone().into_keypair()?;
    ///
    /// let sig = SignatureBuilder::new(SignatureType::SubkeyBinding)
    ///     .set_key_flags(&KeyFlags::empty().set_transport_encryption())?
    ///     .sign_subkey_binding(&mut pk_signer, &pk, &subkey)?;
    ///
    /// let cert = cert.merge_packets(vec![Packet::SecretSubkey(subkey),
    ///                                    sig.into()])?;
    ///
    /// assert_eq!(cert.with_policy(p, None)?.keys().count(), 2);
    /// # Ok(())
    /// # }
    /// ```
    pub fn sign_subkey_binding<P, Q>(mut self, signer: &mut dyn Signer,
                                     primary: &Key<P, key::PrimaryRole>,
                                     subkey: &Key<Q, key::SubordinateRole>)
        -> Result<Signature>
        where P: key::KeyParts,
              Q: key::KeyParts,
    {
        match self.typ {
            SignatureType::SubkeyBinding => (),
            SignatureType::SubkeyRevocation => (),
            SignatureType::Unknown(_) => (),
            _ => return Err(Error::UnsupportedSignatureType(self.typ).into()),
        }

        self = self.pre_sign(signer)?;

        let digest = Signature::hash_subkey_binding(&self, primary, subkey)?;

        self.sign(signer, digest)
    }

    /// Generates a primary key binding signature.
    ///
    /// A [primary key binding signature], also referred to as a back
    /// signature or backsig, is a signature over the primary key and
    /// a subkey, which is made by the subkey.  This signature is a
    /// statement by the subkey that it belongs to the primary key.
    /// That is, it binds the certificate to the subkey.  It is
    /// normally stored in the subkey binding signature (see
    /// [`SignatureBuilder::sign_subkey_binding`]) in the [`Embedded
    /// Signature`] subpacket (set using
    /// [`SignatureBuilder::set_embedded_signature`]).
    ///
    ///   [primary key binding signature]: https://tools.ietf.org/html/rfc4880#section-5.2.1
    ///   [`SignatureBuilder::sign_subkey_binding`]: #method.sign_subkey_binding
    ///   [`Embedded Signature`]: https://tools.ietf.org/html/rfc4880#section-5.2.3.26
    ///   [`SignatureBuilder::set_embedded_signature`]: #method.set_embedded_signature
    ///
    /// All subkeys that make signatures of any sort (signature
    /// subkeys, certification subkeys, and authentication subkeys)
    /// must include this signature in their binding signature.  This
    /// signature ensures that an attacker (Mallory) can't claim
    /// someone else's (Alice's) signing key by just creating a subkey
    /// binding signature.  If that were the case, anyone who has
    /// Mallory's certificate could be tricked into thinking that
    /// Mallory made signatures that were actually made by Alice.
    /// This signature prevents this attack, because it proves that
    /// the person who controls the private key for the primary key
    /// also controls the private key for the subkey and therefore
    /// intended that the subkey be associated with the primary key.
    /// Thus, although Mallory controls his own primary key and can
    /// issue a subkey binding signature for Alice's signing key, he
    /// doesn't control her signing key, and therefore can't create a
    /// valid backsig.
    ///
    /// A primary key binding signature is not needed for
    /// encryption-capable subkeys.  This is firstly because
    /// encryption-capable keys cannot make signatures.  But also
    /// because an attacker doesn't gain anything by adopting an
    /// encryption-capable subkey: without the private key material,
    /// they still can't read the message's content.
    ///
    /// This function checks that the [signature type] (passed to
    /// [`SignatureBuilder::new`], set via
    /// [`SignatureBuilder::set_type`], or copied when using
    /// `SignatureBuilder::From`) is
    /// [`SignatureType::PrimaryKeyBinding`], or
    /// [`SignatureType::Unknown`].
    ///
    ///   [signature type]: ../../types/enum.SignatureType.html
    ///   [`SignatureBuilder::new`]: #method.new
    ///   [`SignatureBuilder::set_type`]: #method.set_type
    ///   [`SignatureType::PrimaryKeyBinding`]: ../../types/enum.SignatureType.html#variant.PrimaryKeyBinding
    ///   [`SignatureType::Unknown`]: ../../types/enum.SignatureType.html#variant.Unknown
    ///
    /// The [`Signature`]'s public-key algorithm field is set to the
    /// algorithm used by `signer`.
    ///
    ///   [`Signature`]: ../enum.Signature.html
    ///
    /// If neither an [`Issuer`] subpacket (set using
    /// [`SignatureBuilder::set_issuer`], for instance) nor an
    /// [`Issuer Fingerprint`] subpacket (set using
    /// [`SignatureBuilder::set_issuer_fingerprint`], for instance) is
    /// set, they are both added to the new `Signature`'s unhashed
    /// subpacket area and set to the `signer`'s `KeyID` and
    /// `Fingerprint`, respectively.
    ///
    ///   [`Issuer`]: https://tools.ietf.org/html/rfc4880#section-5.2.3.5
    ///   [`SignatureBuilder::set_issuer`]: #method.set_issuer
    ///   [`Issuer Fingerprint`]: https://www.ietf.org/id/draft-ietf-openpgp-rfc4880bis-09.html#section-5.2.3.28
    ///   [`SignatureBuilder::set_issuer_fingerprint`]: #method.set_issuer_fingerprint
    ///
    /// Likewise, a [`Signature Creation Time`] subpacket set to the
    /// current time is added to the hashed area if the `Signature
    /// Creation Time` subpacket hasn't been set using, for instance,
    /// the [`set_signature_creation_time`] method or the
    /// [`preserve_signature_creation_time`] method.
    ///
    ///   [`Signature Creation Time`]: https://tools.ietf.org/html/rfc4880#section-5.2.3.4
    ///   [`set_signature_creation_time`]: #method.set_signature_creation_time
    ///   [`preserve_signature_creation_time`]: #method.preserve_signature_creation_time
    ///
    /// # Examples
    ///
    /// Add a new signing-capable subkey to an existing certificate.
    /// Because we are adding a signing-capable subkey, the binding
    /// signature needs to include a backsig.
    ///
    /// ```
    /// use sequoia_openpgp as openpgp;
    /// use openpgp::cert::prelude::*;
    /// use openpgp::packet::prelude::*;
    /// use openpgp::policy::StandardPolicy;
    /// use openpgp::types::KeyFlags;
    /// use openpgp::types::SignatureType;
    ///
    /// # fn main() -> openpgp::Result<()> {
    /// let p = &StandardPolicy::new();
    ///
    /// let (cert, _) = CertBuilder::new().generate()?;
    /// # assert_eq!(cert.keys().count(), 1);
    ///
    /// let pk = cert.primary_key().key().clone().parts_into_secret()?;
    /// // Derive a signer.
    /// let mut pk_signer = pk.clone().into_keypair()?;
    ///
    /// // Generate a signing subkey.
    /// let mut subkey: Key<_, _> = Key4::generate_rsa(3072)?.into();
    /// // Derive a signer.
    /// let mut sk_signer = subkey.clone().into_keypair()?;
    ///
    /// let sig = SignatureBuilder::new(SignatureType::SubkeyBinding)
    ///     .set_key_flags(&KeyFlags::empty().set_signing())?
    ///     // The backsig.  This is essential for subkeys that create signatures!
    ///     .set_embedded_signature(
    ///         SignatureBuilder::new(SignatureType::PrimaryKeyBinding)
    ///             .sign_primary_key_binding(&mut sk_signer, &pk, &subkey)?)?
    ///     .sign_subkey_binding(&mut pk_signer, &pk, &subkey)?;
    ///
    /// let cert = cert.merge_packets(vec![Packet::SecretSubkey(subkey),
    ///                                    sig.into()])?;
    ///
    /// assert_eq!(cert.with_policy(p, None)?.keys().count(), 2);
    /// # assert_eq!(cert.bad_signatures().len(), 0);
    /// # Ok(())
    /// # }
    /// ```
    pub fn sign_primary_key_binding<P, Q>(mut self,
                                          subkey_signer: &mut dyn Signer,
                                          primary: &Key<P, key::PrimaryRole>,
                                          subkey: &Key<Q, key::SubordinateRole>)
        -> Result<Signature>
        where P: key::KeyParts,
              Q: key::KeyParts,
    {
        match self.typ {
            SignatureType::PrimaryKeyBinding => (),
            SignatureType::Unknown(_) => (),
            _ => return Err(Error::UnsupportedSignatureType(self.typ).into()),
        }

        self = self.pre_sign(subkey_signer)?;

        let digest =
            Signature::hash_primary_key_binding(&self, primary, subkey)?;

        self.sign(subkey_signer, digest)
    }


    /// Generates a User Attribute binding signature.
    ///
    /// A User Attribute binding signature or certification, a type of
    /// [User ID certification], is a signature over a User Attribute
    /// and a Primary Key.  It asserts that the signer is convinced
    /// that the User Attribute should be associated with the
    /// Certificate, i.e., that the binding is authentic.
    ///
    ///   [User ID certification]: https://tools.ietf.org/html/rfc4880#section-5.2.1
    ///
    /// OpenPGP has four types of User Attribute certifications.  They
    /// are intended to express the degree of the signer's conviction.
    /// In practice, the `Positive Certification` type is used for
    /// self-signatures, and the `Generic Certification` is used for
    /// third-party certifications; the other types are not normally
    /// used.
    ///
    /// This function checks that the [signature type] (passed to
    /// [`SignatureBuilder::new`], set via
    /// [`SignatureBuilder::set_type`], or copied when using
    /// `SignatureBuilder::From`) is [`GenericCertification`],
    /// [`PersonaCertification`], [`CasualCertification`],
    /// [`PositiveCertification`], [`CertificationRevocation`], or
    /// [`SignatureType::Unknown`].
    ///
    ///   [signature type]: ../../types/enum.SignatureType.html
    ///   [`SignatureBuilder::new`]: #method.new
    ///   [`SignatureBuilder::set_type`]: #method.set_type
    ///   [`GenericCertification`]: ../../types/enum.SignatureType.html#variant.GenericCertification
    ///   [`PersonaCertification`]: ../../types/enum.SignatureType.html#variant.PersonaCertification
    ///   [`CasualCertification`]: ../../types/enum.SignatureType.html#variant.CasualCertification
    ///   [`PositiveCertification`]: ../../types/enum.SignatureType.html#variant.PositiveCertification
    ///   [`CertificationRevocation`]: ../../types/enum.SignatureType.html#variant.CertificationRevocation
    ///   [`SignatureType::Unknown`]: ../../types/enum.SignatureType.html#variant.Unknown
    ///
    /// The [`Signature`]'s public-key algorithm field is set to the
    /// algorithm used by `signer`.
    ///
    ///   [`Signature`]: ../enum.Signature.html
    ///
    /// If neither an [`Issuer`] subpacket (set using
    /// [`SignatureBuilder::set_issuer`], for instance) nor an
    /// [`Issuer Fingerprint`] subpacket (set using
    /// [`SignatureBuilder::set_issuer_fingerprint`], for instance) is
    /// set, they are both added to the new `Signature`'s unhashed
    /// subpacket area and set to the `signer`'s `KeyID` and
    /// `Fingerprint`, respectively.
    ///
    ///   [`Issuer`]: https://tools.ietf.org/html/rfc4880#section-5.2.3.5
    ///   [`SignatureBuilder::set_issuer`]: #method.set_issuer
    ///   [`Issuer Fingerprint`]: https://www.ietf.org/id/draft-ietf-openpgp-rfc4880bis-09.html#section-5.2.3.28
    ///   [`SignatureBuilder::set_issuer_fingerprint`]: #method.set_issuer_fingerprint
    ///
    /// Likewise, a [`Signature Creation Time`] subpacket set to the
    /// current time is added to the hashed area if the `Signature
    /// Creation Time` subpacket hasn't been set using, for instance,
    /// the [`set_signature_creation_time`] method or the
    /// [`preserve_signature_creation_time`] method.
    ///
    ///   [`Signature Creation Time`]: https://tools.ietf.org/html/rfc4880#section-5.2.3.4
    ///   [`set_signature_creation_time`]: #method.set_signature_creation_time
    ///   [`preserve_signature_creation_time`]: #method.preserve_signature_creation_time
    ///
    /// # Examples
    ///
    /// Add a new User Attribute to an existing certificate:
    ///
    /// ```
    /// use sequoia_openpgp as openpgp;
    /// use openpgp::cert::prelude::*;
    /// use openpgp::packet::prelude::*;
    /// use openpgp::policy::StandardPolicy;
    /// use openpgp::types::SignatureType;
    /// # use openpgp::packet::user_attribute::{Subpacket, Image};
    ///
    /// # fn main() -> openpgp::Result<()> {
    /// let p = &StandardPolicy::new();
    ///
    /// # // Add a bare user attribute.
    /// # let ua = UserAttribute::new(&[
    /// #     Subpacket::Image(
    /// #         Image::Private(100, vec![0, 1, 2].into_boxed_slice())),
    /// # ])?;
    /// #
    /// let (cert, _) = CertBuilder::new().generate()?;
    /// # assert_eq!(cert.user_attributes().count(), 0);
    ///
    /// // Add a user attribute.
    ///
    /// // Get a usable (alive, non-revoked) certification key.
    /// let key : &Key<_, _> = cert
    ///     .keys().with_policy(p, None)
    ///     .for_certification().alive().revoked(false).nth(0).unwrap().key();
    /// // Derive a signer.
    /// let mut signer = key.clone().parts_into_secret()?.into_keypair()?;
    ///
    /// let pk = cert.primary_key().key();
    ///
    /// let sig = SignatureBuilder::new(SignatureType::PositiveCertification)
    ///     .sign_user_attribute_binding(&mut signer, pk, &ua)?;
    ///
    /// // Verify it.
    /// sig.verify_user_attribute_binding(signer.public(), pk, &ua)?;
    ///
    /// let cert = cert.merge_packets(vec![Packet::from(ua), sig.into()])?;
    /// assert_eq!(cert.with_policy(p, None)?.user_attributes().count(), 1);
    /// # Ok(())
    /// # }
    /// ```
    pub fn sign_user_attribute_binding<P>(mut self, signer: &mut dyn Signer,
                                          key: &Key<P, key::PrimaryRole>,
                                          ua: &UserAttribute)
        -> Result<Signature>
        where P: key::KeyParts,
    {
        match self.typ {
            SignatureType::GenericCertification => (),
            SignatureType::PersonaCertification => (),
            SignatureType::CasualCertification => (),
            SignatureType::PositiveCertification => (),
            SignatureType::CertificationRevocation => (),
            SignatureType::Unknown(_) => (),
            _ => return Err(Error::UnsupportedSignatureType(self.typ).into()),
        }

        self = self.pre_sign(signer)?;

        let digest =
            Signature::hash_user_attribute_binding(&self, key, ua)?;

        self.sign(signer, digest)
    }

    /// Generates a signature.
    ///
    /// This is a low-level function.  Normally, you'll want to use
    /// one of the higher-level functions, like
    /// [`SignatureBuilder::sign_userid_binding`].  But, this function
    /// is useful if you want to create a [`Signature`] for an
    /// unsupported signature type.
    ///
    ///   [`SignatureBuilder::sign_userid_binding`]: #method.sign_userid_binding
    ///   [`Signature`]: ../enum.Signature.html
    ///
    /// The `Signature`'s public-key algorithm field is set to the
    /// algorithm used by `signer`.
    ///
    /// If neither an [`Issuer`] subpacket (set using
    /// [`SignatureBuilder::set_issuer`], for instance) nor an
    /// [`Issuer Fingerprint`] subpacket (set using
    /// [`SignatureBuilder::set_issuer_fingerprint`], for instance) is
    /// set, they are both added to the new `Signature`'s unhashed
    /// subpacket area and set to the `signer`'s `KeyID` and
    /// `Fingerprint`, respectively.
    ///
    ///   [`Issuer`]: https://tools.ietf.org/html/rfc4880#section-5.2.3.5
    ///   [`SignatureBuilder::set_issuer`]: #method.set_issuer
    ///   [`Issuer Fingerprint`]: https://www.ietf.org/id/draft-ietf-openpgp-rfc4880bis-09.html#section-5.2.3.28
    ///   [`SignatureBuilder::set_issuer_fingerprint`]: #method.set_issuer_fingerprint
    ///
    /// Likewise, a [`Signature Creation Time`] subpacket set to the
    /// current time is added to the hashed area if the `Signature
    /// Creation Time` subpacket hasn't been set using, for instance,
    /// the [`set_signature_creation_time`] method or the
    /// [`preserve_signature_creation_time`] method.
    ///
    ///   [`Signature Creation Time`]: https://tools.ietf.org/html/rfc4880#section-5.2.3.4
    ///   [`set_signature_creation_time`]: #method.set_signature_creation_time
    ///   [`preserve_signature_creation_time`]: #method.preserve_signature_creation_time
    pub fn sign_hash(mut self, signer: &mut dyn Signer,
                     mut hash: hash::Context)
        -> Result<Signature>
    {
        self.hash_algo = hash.algo();

        self = self.pre_sign(signer)?;

        self.hash(&mut hash);
        let mut digest = vec![0u8; hash.digest_size()];
        hash.digest(&mut digest);

        self.sign(signer, digest)
    }

    /// Signs a message.
    ///
    /// Normally, you'll want to use the [streaming `Signer`] to sign
    /// a message.
    ///
    ///  [streaming `Signer`]: ../../serialize/stream/struct.Signer.html
    ///
    /// OpenPGP supports two types of signatures over messages: binary
    /// and text.  The text version normalizes line endings.  But,
    /// since nearly all software today can deal with both Unix and
    /// DOS line endings, it is better to just use the binary version
    /// even when dealing with text.  This avoids any possible
    /// ambiguity.
    ///
    /// This function checks that the [signature type] (passed to
    /// [`SignatureBuilder::new`], set via
    /// [`SignatureBuilder::set_type`], or copied when using
    /// `SignatureBuilder::From`) is [`Binary`], [`Text`], or
    /// [`SignatureType::Unknown`].
    ///
    ///   [signature type]: ../../types/enum.SignatureType.html
    ///   [`SignatureBuilder::new`]: #method.new
    ///   [`SignatureBuilder::set_type`]: #method.set_type
    ///   [`Binary`]: ../../types/enum.SignatureType.html#variant.Binary
    ///   [`Text`]: ../../types/enum.SignatureType.html#variant.Text
    ///   [`SignatureType::Unknown`]: ../../types/enum.SignatureType.html#variant.Unknown
    ///
    /// The [`Signature`]'s public-key algorithm field is set to the
    /// algorithm used by `signer`.
    ///
    ///   [`Signature`]: ../enum.Signature.html
    ///
    /// If neither an [`Issuer`] subpacket (set using
    /// [`SignatureBuilder::set_issuer`], for instance) nor an
    /// [`Issuer Fingerprint`] subpacket (set using
    /// [`SignatureBuilder::set_issuer_fingerprint`], for instance) is
    /// set, they are both added to the new `Signature`'s unhashed
    /// subpacket area and set to the `signer`'s `KeyID` and
    /// `Fingerprint`, respectively.
    ///
    ///   [`Issuer`]: https://tools.ietf.org/html/rfc4880#section-5.2.3.5
    ///   [`SignatureBuilder::set_issuer`]: #method.set_issuer
    ///   [`Issuer Fingerprint`]: https://www.ietf.org/id/draft-ietf-openpgp-rfc4880bis-09.html#section-5.2.3.28
    ///   [`SignatureBuilder::set_issuer_fingerprint`]: #method.set_issuer_fingerprint
    ///
    /// Likewise, a [`Signature Creation Time`] subpacket set to the
    /// current time is added to the hashed area if the `Signature
    /// Creation Time` subpacket hasn't been set using, for instance,
    /// the [`set_signature_creation_time`] method or the
    /// [`preserve_signature_creation_time`] method.
    ///
    ///   [`Signature Creation Time`]: https://tools.ietf.org/html/rfc4880#section-5.2.3.4
    ///   [`set_signature_creation_time`]: #method.set_signature_creation_time
    ///   [`preserve_signature_creation_time`]: #method.preserve_signature_creation_time
    ///
    /// # Examples
    ///
    /// Signs a document.  For large messages, you should use the
    /// [streaming `Signer`], which streams the message's content.
    ///
    ///  [streaming `Signer`]: ../../serialize/stream/struct.Signer.html
    ///
    /// ```
    /// use sequoia_openpgp as openpgp;
    /// use openpgp::cert::prelude::*;
    /// use openpgp::packet::prelude::*;
    /// use openpgp::policy::StandardPolicy;
    /// use openpgp::types::SignatureType;
    ///
    /// # fn main() -> openpgp::Result<()> {
    /// let p = &StandardPolicy::new();
    ///
    /// let (cert, _) = CertBuilder::new().generate()?;
    ///
    /// // Get a usable (alive, non-revoked) certification key.
    /// let key : &Key<_, _> = cert
    ///     .keys().with_policy(p, None)
    ///     .for_certification().alive().revoked(false).nth(0).unwrap().key();
    /// // Derive a signer.
    /// let mut signer = key.clone().parts_into_secret()?.into_keypair()?;
    ///
    /// // For large messages, you should use openpgp::serialize::stream::Signer,
    /// // which streams the message's content.
    /// let msg = b"Hello, world!";
    /// let sig = SignatureBuilder::new(SignatureType::Binary)
    ///     .sign_message(&mut signer, msg)?;
    ///
    /// // Verify it.
    /// sig.verify_message(signer.public(), msg)?;
    /// # Ok(())
    /// # }
    /// ```
    pub fn sign_message<M>(mut self, signer: &mut dyn Signer, msg: M)
        -> Result<Signature>
        where M: AsRef<[u8]>
    {
        match self.typ {
            SignatureType::Binary => (),
            SignatureType::Text => (),
            SignatureType::Unknown(_) => (),
            _ => return Err(Error::UnsupportedSignatureType(self.typ).into()),
        }

        // Hash the message
        let mut hash = self.hash_algo.context()?;
        hash.update(msg.as_ref());

        self = self.pre_sign(signer)?;

        self.hash(&mut hash);
        let mut digest = vec![0u8; hash.digest_size()];
        hash.digest(&mut digest);

        self.sign(signer, digest)
    }

    fn pre_sign(mut self, signer: &dyn Signer) -> Result<Self> {
        self.pk_algo = signer.public().pk_algo();

        // Set the creation time.
        if ! self.overrode_creation_time {
            self = self.set_signature_creation_time(
                std::time::SystemTime::now())?;
        }

        // Make sure we have an issuer packet.
        if self.issuer().is_none() && self.issuer_fingerprint().is_none() {
            self = self.set_issuer(signer.public().keyid())?
                .set_issuer_fingerprint(signer.public().fingerprint())?;
        }

        self.sort();

        Ok(self)
    }

    fn sign(self, signer: &mut dyn Signer, digest: Vec<u8>)
        -> Result<Signature>
    {
        let mpis = signer.sign(self.hash_algo, &digest)?;

        Ok(Signature4 {
            common: Default::default(),
            fields: self.fields,
            digest_prefix: [digest[0], digest[1]],
            mpis,
            computed_digest: Some(digest),
            level: 0,
        }.into())
    }
}

impl From<Signature> for SignatureBuilder {
    fn from(sig: Signature) -> Self {
        match sig {
            Signature::V4(sig) => sig.into(),
            Signature::__Nonexhaustive => unreachable!(),
        }
    }
}

impl From<Signature4> for SignatureBuilder {
    fn from(sig: Signature4) -> Self {
        let mut fields = sig.fields;

        let creation_time = fields.signature_creation_time();

        fields.hashed_area_mut().remove_all(SubpacketTag::SignatureCreationTime);
        fields.hashed_area_mut().remove_all(SubpacketTag::Issuer);
        fields.hashed_area_mut().remove_all(SubpacketTag::IssuerFingerprint);

        fields.unhashed_area_mut().remove_all(SubpacketTag::SignatureCreationTime);
        fields.unhashed_area_mut().remove_all(SubpacketTag::Issuer);
        fields.unhashed_area_mut().remove_all(SubpacketTag::IssuerFingerprint);

        SignatureBuilder {
            overrode_creation_time: false,
            original_creation_time: creation_time,
            fields: fields,
        }
    }
}

/// Holds a signature packet.
///
/// Signature packets are used both for certification purposes as well
/// as for document signing purposes.
///
/// See [Section 5.2 of RFC 4880] for details.
///
///   [Section 5.2 of RFC 4880]: https://tools.ietf.org/html/rfc4880#section-5.2
// Note: we can't derive PartialEq, because it includes the cached data.
#[derive(Clone)]
pub struct Signature4 {
    /// CTB packet header fields.
    pub(crate) common: packet::Common,

    /// Fields as configured using the SignatureBuilder.
    pub(crate) fields: SignatureFields,

    /// Lower 16 bits of the signed hash value.
    digest_prefix: [u8; 2],
    /// Signature MPIs.
    mpis: mpi::Signature,

    /// When used in conjunction with a one-pass signature, this is the
    /// hash computed over the enclosed message.
    computed_digest: Option<Vec<u8>>,

    /// Signature level.
    ///
    /// A level of 0 indicates that the signature is directly over the
    /// data, a level of 1 means that the signature is a notarization
    /// over all level 0 signatures and the data, and so on.
    level: usize,
}

impl fmt::Debug for Signature4 {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.debug_struct("Signature4")
            .field("version", &self.version())
            .field("typ", &self.typ())
            .field("pk_algo", &self.pk_algo())
            .field("hash_algo", &self.hash_algo())
            .field("hashed_area", self.hashed_area())
            .field("unhashed_area", self.unhashed_area())
            .field("digest_prefix",
                   &crate::fmt::to_hex(&self.digest_prefix, false))
            .field("computed_digest",
                   &if let Some(ref hash) = self.computed_digest {
                       Some(crate::fmt::to_hex(&hash[..], false))
                   } else {
                       None
                   })
            .field("level", &self.level)
            .field("mpis", &self.mpis)
            .finish()
    }
}

impl PartialEq for Signature4 {
    /// This method tests for self and other values to be equal, and
    /// is used by ==.
    ///
    /// Note: We ignore the unhashed subpacket area when comparing
    /// signatures.  This prevents a malicious party to take valid
    /// signatures, add subpackets to the unhashed area, yielding
    /// valid but distinct signatures.
    ///
    /// The problem we are trying to avoid here is signature spamming.
    /// Ignoring the unhashed subpackets means that we can deduplicate
    /// signatures using this predicate.
    fn eq(&self, other: &Signature4) -> bool {
        self.mpis == other.mpis
            && self.fields == other.fields
            && self.digest_prefix == other.digest_prefix
    }
}

impl Eq for Signature4 {}

impl std::hash::Hash for Signature4 {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        use std::hash::Hash as StdHash;
        StdHash::hash(&self.mpis, state);
        StdHash::hash(&self.fields, state);
        self.digest_prefix.hash(state);
    }
}

impl Signature4 {
    /// Creates a new signature packet.
    ///
    /// If you want to sign something, consider using the [`SignatureBuilder`]
    /// interface.
    ///
    /// [`SignatureBuilder`]: struct.SignatureBuilder.html
    pub fn new(typ: SignatureType, pk_algo: PublicKeyAlgorithm,
               hash_algo: HashAlgorithm, hashed_area: SubpacketArea,
               unhashed_area: SubpacketArea,
               digest_prefix: [u8; 2],
               mpis: mpi::Signature) -> Self {
        Signature4 {
            common: Default::default(),
            fields: SignatureFields {
                version: 4,
                typ,
                pk_algo,
                hash_algo,
                subpackets: SubpacketAreas::new(hashed_area, unhashed_area),
            },
            digest_prefix,
            mpis,
            computed_digest: None,
            level: 0,
        }
    }

    /// Gets the hash prefix.
    pub fn digest_prefix(&self) -> &[u8; 2] {
        &self.digest_prefix
    }

    /// Sets the hash prefix.
    #[allow(dead_code)]
    pub(crate) fn set_digest_prefix(&mut self, prefix: [u8; 2]) -> [u8; 2] {
        ::std::mem::replace(&mut self.digest_prefix, prefix)
    }

    /// Gets the signature packet's MPIs.
    pub fn mpis(&self) -> &mpi::Signature {
        &self.mpis
    }

    /// Sets the signature packet's MPIs.
    #[allow(dead_code)]
    pub(crate) fn set_mpis(&mut self, mpis: mpi::Signature) -> mpi::Signature
    {
        ::std::mem::replace(&mut self.mpis, mpis)
    }

    /// Gets the computed hash value.
    pub fn computed_digest(&self) -> Option<&[u8]> {
        self.computed_digest.as_ref().map(|d| &d[..])
    }

    /// Sets the computed hash value.
    pub(crate) fn set_computed_digest(&mut self, hash: Option<Vec<u8>>)
        -> Option<Vec<u8>>
    {
        ::std::mem::replace(&mut self.computed_digest, hash)
    }

    /// Gets the signature level.
    ///
    /// A level of 0 indicates that the signature is directly over the
    /// data, a level of 1 means that the signature is a notarization
    /// over all level 0 signatures and the data, and so on.
    pub fn level(&self) -> usize {
        self.level
    }

    /// Sets the signature level.
    ///
    /// A level of 0 indicates that the signature is directly over the
    /// data, a level of 1 means that the signature is a notarization
    /// over all level 0 signatures and the data, and so on.
    pub(crate) fn set_level(&mut self, level: usize) -> usize {
        ::std::mem::replace(&mut self.level, level)
    }

    /// Tests whether or not this signature is exportable.
    pub fn exportable(&self) -> Result<()> {
        if ! self.exportable_certification().unwrap_or(true) {
            return Err(Error::InvalidOperation(
                "Cannot export non-exportable certification".into()).into());
        }

        if self.revocation_keys().any(|r| r.sensitive()) {
            return Err(Error::InvalidOperation(
                "Cannot export signature with sensitive designated revoker"
                    .into()).into());
        }

        Ok(())
    }
}

impl crate::packet::Signature {
    /// Collects all the issuers.
    ///
    /// A signature can contain multiple hints as to who issued the
    /// signature.
    pub fn get_issuers(&self) -> Vec<crate::KeyHandle> {
        use crate::packet::signature::subpacket:: SubpacketValue;

        let mut issuers: Vec<_> =
            self.hashed_area().iter()
            .chain(self.unhashed_area().iter())
            .filter_map(|subpacket| {
                match subpacket.value() {
                    SubpacketValue::Issuer(i) => Some(i.into()),
                    SubpacketValue::IssuerFingerprint(i) => Some(i.into()),
                    _ => None,
                }
            })
            .collect();

        // Sort the issuers so that the fingerprints come first.
        issuers.sort_by(|a, b| {
            use crate::KeyHandle::*;
            use std::cmp::Ordering::*;
            match (a, b) {
                (Fingerprint(_), Fingerprint(_)) => Equal,
                (KeyID(_), Fingerprint(_)) => Greater,
                (Fingerprint(_), KeyID(_)) => Less,
                (KeyID(_), KeyID(_)) => Equal,
            }
        });
        issuers
    }

    /// Compares Signatures ignoring the unhashed subpacket area.
    ///
    /// We ignore the unhashed subpacket area when comparing
    /// signatures.  This prevents a malicious party to take valid
    /// signatures, add subpackets to the unhashed area, yielding
    /// valid but distinct signatures.
    ///
    /// The problem we are trying to avoid here is signature spamming.
    /// Ignoring the unhashed subpackets means that we can deduplicate
    /// signatures using this predicate.
    pub fn normalized_eq(&self, other: &Signature) -> bool {
        self.mpis() == other.mpis()
            && self.version() == other.version()
            && self.typ() == other.typ()
            && self.pk_algo() == other.pk_algo()
            && self.hash_algo() == other.hash_algo()
            && self.hashed_area() == other.hashed_area()
            && self.digest_prefix() == other.digest_prefix()
    }

    /// Normalizes the signature.
    ///
    /// This function normalizes the *unhashed* signature subpackets.
    /// It removes all but the following self-authenticating
    /// subpackets:
    ///
    ///   - `SubpacketValue::Issuer`
    ///   - `SubpacketValue::IssuerFingerprint`
    ///   - `SubpacketValue::EmbeddedSignature`
    pub fn normalize(&self) -> Self {
        use crate::packet::signature::subpacket::SubpacketTag::*;
        let mut sig = self.clone();
        {
            let area = sig.unhashed_area_mut();
            area.clear();

            for spkt in self.unhashed_area().iter()
                .filter(|s| s.tag() == Issuer
                        || s.tag() == IssuerFingerprint
                        || s.tag() == EmbeddedSignature)
            {
                area.add(spkt.clone())
                    .expect("it did fit into the old area");
            }
        }
        sig
    }
}

/// Verification-related functionality.
///
/// <a name="verification-functions"></a>
impl Signature {
    /// Verifies the signature against `hash`.
    ///
    /// Note: Due to limited context, this only verifies the
    /// cryptographic signature and checks that the key predates the
    /// signature.  Further constraints on the signature, like
    /// creation and expiration time, or signature revocations must be
    /// checked by the caller.
    ///
    /// Likewise, this function does not check whether `key` can made
    /// valid signatures; it is up to the caller to make sure the key
    /// is not revoked, not expired, has a valid self-signature, has a
    /// subkey binding signature (if appropriate), has the signing
    /// capability, etc.
    pub fn verify_digest<P, R, D>(&self, key: &Key<P, R>, digest: D)
        -> Result<()>
        where P: key::KeyParts,
              R: key::KeyRole,
              D: AsRef<[u8]>,
    {
        if let Some(creation_time) = self.signature_creation_time() {
            if creation_time < key.creation_time() {
                return Err(Error::BadSignature(
                    format!("Signature (created {:?}) predates key ({:?})",
                            creation_time, key.creation_time())).into());
            }
        } else {
            return Err(Error::BadSignature(
                "Signature has no creation time subpacket".into()).into());
        }

        key.verify(self, digest.as_ref())
    }

    /// Verifies the signature over text or binary documents using
    /// `key`.
    ///
    /// Note: Due to limited context, this only verifies the
    /// cryptographic signature, checks the signature's type, and
    /// checks that the key predates the signature.  Further
    /// constraints on the signature, like creation and expiration
    /// time, or signature revocations must be checked by the caller.
    ///
    /// Likewise, this function does not check whether `key` can make
    /// valid signatures; it is up to the caller to make sure the key
    /// is not revoked, not expired, has a valid self-signature, has a
    /// subkey binding signature (if appropriate), has the signing
    /// capability, etc.
    pub fn verify<P, R>(&self, key: &Key<P, R>) -> Result<()>
        where P: key::KeyParts,
              R: key::KeyRole,
    {
        if !(self.typ() == SignatureType::Binary
             || self.typ() == SignatureType::Text) {
            return Err(Error::UnsupportedSignatureType(self.typ()).into());
        }

        if let Some(ref hash) = self.computed_digest {
            self.verify_digest(key, hash)
        } else {
            Err(Error::BadSignature("Hash not computed.".to_string()).into())
        }
    }

    /// Verifies the standalone signature using `key`.
    ///
    /// Note: Due to limited context, this only verifies the
    /// cryptographic signature, checks the signature's type, and
    /// checks that the key predates the signature.  Further
    /// constraints on the signature, like creation and expiration
    /// time, or signature revocations must be checked by the caller.
    ///
    /// Likewise, this function does not check whether `key` can make
    /// valid signatures; it is up to the caller to make sure the key
    /// is not revoked, not expired, has a valid self-signature, has a
    /// subkey binding signature (if appropriate), has the signing
    /// capability, etc.
    pub fn verify_standalone<P, R>(&self, key: &Key<P, R>) -> Result<()>
        where P: key::KeyParts,
              R: key::KeyRole,
    {
        if self.typ() != SignatureType::Standalone {
            return Err(Error::UnsupportedSignatureType(self.typ()).into());
        }

        // Standalone signatures are like binary-signatures over the
        // zero-sized string.
        let digest = Signature::hash_standalone(self)?;
        self.verify_digest(key, &digest[..])
    }

    /// Verifies the timestamp signature using `key`.
    ///
    /// Note: Due to limited context, this only verifies the
    /// cryptographic signature, checks the signature's type, and
    /// checks that the key predates the signature.  Further
    /// constraints on the signature, like creation and expiration
    /// time, or signature revocations must be checked by the caller.
    ///
    /// Likewise, this function does not check whether `key` can make
    /// valid signatures; it is up to the caller to make sure the key
    /// is not revoked, not expired, has a valid self-signature, has a
    /// subkey binding signature (if appropriate), has the signing
    /// capability, etc.
    pub fn verify_timestamp<P, R>(&self, key: &Key<P, R>) -> Result<()>
        where P: key::KeyParts,
              R: key::KeyRole,
    {
        if self.typ() != SignatureType::Timestamp {
            return Err(Error::UnsupportedSignatureType(self.typ()).into());
        }

        // Timestamp signatures are like binary-signatures over the
        // zero-sized string.
        let digest = Signature::hash_timestamp(self)?;
        self.verify_digest(key, &digest[..])
    }

    /// Verifies the direct key signature.
    ///
    /// `self` is the direct key signature, `signer` is the
    /// key that allegedly made the signature, and `pk` is the primary
    /// key.
    ///
    /// For a self-signature, `signer` and `pk` will be the same.
    ///
    /// Note: Due to limited context, this only verifies the
    /// cryptographic signature, checks the signature's type, and
    /// checks that the key predates the signature.  Further
    /// constraints on the signature, like creation and expiration
    /// time, or signature revocations must be checked by the caller.
    ///
    /// Likewise, this function does not check whether `signer` can
    /// made valid signatures; it is up to the caller to make sure the
    /// key is not revoked, not expired, has a valid self-signature,
    /// has a subkey binding signature (if appropriate), has the
    /// signing capability, etc.
    pub fn verify_direct_key<P, Q, R>(&self,
                                      signer: &Key<P, R>,
                                      pk: &Key<Q, key::PrimaryRole>)
        -> Result<()>
        where P: key::KeyParts,
              Q: key::KeyParts,
              R: key::KeyRole,
    {
        if self.typ() != SignatureType::DirectKey {
            return Err(Error::UnsupportedSignatureType(self.typ()).into());
        }

        let hash = Signature::hash_direct_key(self, pk)?;
        self.verify_digest(signer, &hash[..])
    }

    /// Verifies the primary key revocation certificate.
    ///
    /// `self` is the primary key revocation certificate, `signer` is
    /// the key that allegedly made the signature, and `pk` is the
    /// primary key,
    ///
    /// For a self-signature, `signer` and `pk` will be the same.
    ///
    /// Note: Due to limited context, this only verifies the
    /// cryptographic signature, checks the signature's type, and
    /// checks that the key predates the signature.  Further
    /// constraints on the signature, like creation and expiration
    /// time, or signature revocations must be checked by the caller.
    ///
    /// Likewise, this function does not check whether `signer` can
    /// made valid signatures; it is up to the caller to make sure the
    /// key is not revoked, not expired, has a valid self-signature,
    /// has a subkey binding signature (if appropriate), has the
    /// signing capability, etc.
    pub fn verify_primary_key_revocation<P, Q, R>(&self,
                                                  signer: &Key<P, R>,
                                                  pk: &Key<Q, key::PrimaryRole>)
        -> Result<()>
        where P: key::KeyParts,
              Q: key::KeyParts,
              R: key::KeyRole,
    {
        if self.typ() != SignatureType::KeyRevocation {
            return Err(Error::UnsupportedSignatureType(self.typ()).into());
        }

        let hash = Signature::hash_direct_key(self, pk)?;
        self.verify_digest(signer, &hash[..])
    }

    /// Verifies the subkey binding.
    ///
    /// `self` is the subkey key binding signature, `signer` is the
    /// key that allegedly made the signature, `pk` is the primary
    /// key, and `subkey` is the subkey.
    ///
    /// For a self-signature, `signer` and `pk` will be the same.
    ///
    /// If the signature indicates that this is a `Signing` capable
    /// subkey, then the back signature is also verified.  If it is
    /// missing or can't be verified, then this function returns
    /// false.
    ///
    /// Note: Due to limited context, this only verifies the
    /// cryptographic signature, checks the signature's type, and
    /// checks that the key predates the signature.  Further
    /// constraints on the signature, like creation and expiration
    /// time, or signature revocations must be checked by the caller.
    ///
    /// Likewise, this function does not check whether `signer` can
    /// made valid signatures; it is up to the caller to make sure the
    /// key is not revoked, not expired, has a valid self-signature,
    /// has a subkey binding signature (if appropriate), has the
    /// signing capability, etc.
    pub fn verify_subkey_binding<P, Q, R, S>(
        &self,
        signer: &Key<P, R>,
        pk: &Key<Q, key::PrimaryRole>,
        subkey: &Key<S, key::SubordinateRole>)
        -> Result<()>
        where P: key::KeyParts,
              Q: key::KeyParts,
              R: key::KeyRole,
              S: key::KeyParts,
    {
        if self.typ() != SignatureType::SubkeyBinding {
            return Err(Error::UnsupportedSignatureType(self.typ()).into());
        }

        let hash = Signature::hash_subkey_binding(self, pk, subkey)?;
        self.verify_digest(signer, &hash[..])?;

        // The signature is good, but we may still need to verify the
        // back sig.
        if self.key_flags().map(|kf| kf.for_signing()).unwrap_or(false) {
            if let Some(backsig) = self.embedded_signature() {
                backsig.verify_primary_key_binding(pk, subkey)
            } else {
                Err(Error::BadSignature(
                    "Primary key binding signature missing".into()).into())
            }
        } else {
            // No backsig required.
            Ok(())
        }
    }

    /// Verifies the primary key binding.
    ///
    /// `self` is the primary key binding signature, `pk` is the
    /// primary key, and `subkey` is the subkey.
    ///
    /// Note: Due to limited context, this only verifies the
    /// cryptographic signature, checks the signature's type, and
    /// checks that the key predates the signature.  Further
    /// constraints on the signature, like creation and expiration
    /// time, or signature revocations must be checked by the caller.
    ///
    /// Likewise, this function does not check whether `subkey` can
    /// made valid signatures; it is up to the caller to make sure the
    /// key is not revoked, not expired, has a valid self-signature,
    /// has a subkey binding signature (if appropriate), has the
    /// signing capability, etc.
    pub fn verify_primary_key_binding<P, Q>(
        &self,
        pk: &Key<P, key::PrimaryRole>,
        subkey: &Key<Q, key::SubordinateRole>)
        -> Result<()>
        where P: key::KeyParts,
              Q: key::KeyParts,
    {
        if self.typ() != SignatureType::PrimaryKeyBinding {
            return Err(Error::UnsupportedSignatureType(self.typ()).into());
        }

        let hash = Signature::hash_primary_key_binding(self, pk, subkey)?;
        self.verify_digest(subkey, &hash[..])
    }

    /// Verifies the subkey revocation.
    ///
    /// `self` is the subkey key revocation certificate, `signer` is
    /// the key that allegedly made the signature, `pk` is the primary
    /// key, and `subkey` is the subkey.
    ///
    /// For a self-revocation, `signer` and `pk` will be the same.
    ///
    /// Note: Due to limited context, this only verifies the
    /// cryptographic signature, checks the signature's type, and
    /// checks that the key predates the signature.  Further
    /// constraints on the signature, like creation and expiration
    /// time, or signature revocations must be checked by the caller.
    ///
    /// Likewise, this function does not check whether `signer` can
    /// made valid signatures; it is up to the caller to make sure the
    /// key is not revoked, not expired, has a valid self-signature,
    /// has a subkey binding signature (if appropriate), has the
    /// signing capability, etc.
    pub fn verify_subkey_revocation<P, Q, R, S>(
        &self,
        signer: &Key<P, R>,
        pk: &Key<Q, key::PrimaryRole>,
        subkey: &Key<S, key::SubordinateRole>)
        -> Result<()>
        where P: key::KeyParts,
              Q: key::KeyParts,
              R: key::KeyRole,
              S: key::KeyParts,
    {
        if self.typ() != SignatureType::SubkeyRevocation {
            return Err(Error::UnsupportedSignatureType(self.typ()).into());
        }

        let hash = Signature::hash_subkey_binding(self, pk, subkey)?;
        self.verify_digest(signer, &hash[..])
    }

    /// Verifies the user id binding.
    ///
    /// `self` is the user id binding signature, `signer` is the key
    /// that allegedly made the signature, `pk` is the primary key,
    /// and `userid` is the user id.
    ///
    /// For a self-signature, `signer` and `pk` will be the same.
    ///
    /// Note: Due to limited context, this only verifies the
    /// cryptographic signature, checks the signature's type, and
    /// checks that the key predates the signature.  Further
    /// constraints on the signature, like creation and expiration
    /// time, or signature revocations must be checked by the caller.
    ///
    /// Likewise, this function does not check whether `signer` can
    /// made valid signatures; it is up to the caller to make sure the
    /// key is not revoked, not expired, has a valid self-signature,
    /// has a subkey binding signature (if appropriate), has the
    /// signing capability, etc.
    pub fn verify_userid_binding<P, Q, R>(&self,
                                          signer: &Key<P, R>,
                                          pk: &Key<Q, key::PrimaryRole>,
                                          userid: &UserID)
        -> Result<()>
        where P: key::KeyParts,
              Q: key::KeyParts,
              R: key::KeyRole,
    {
        if !(self.typ() == SignatureType::GenericCertification
             || self.typ() == SignatureType::PersonaCertification
             || self.typ() == SignatureType::CasualCertification
             || self.typ() == SignatureType::PositiveCertification) {
            return Err(Error::UnsupportedSignatureType(self.typ()).into());
        }

        let hash = Signature::hash_userid_binding(self, pk, userid)?;
        self.verify_digest(signer, &hash[..])
    }

    /// Verifies the user id revocation certificate.
    ///
    /// `self` is the revocation certificate, `signer` is the key
    /// that allegedly made the signature, `pk` is the primary key,
    /// and `userid` is the user id.
    ///
    /// For a self-signature, `signer` and `pk` will be the same.
    ///
    /// Note: Due to limited context, this only verifies the
    /// cryptographic signature, checks the signature's type, and
    /// checks that the key predates the signature.  Further
    /// constraints on the signature, like creation and expiration
    /// time, or signature revocations must be checked by the caller.
    ///
    /// Likewise, this function does not check whether `signer` can
    /// made valid signatures; it is up to the caller to make sure the
    /// key is not revoked, not expired, has a valid self-signature,
    /// has a subkey binding signature (if appropriate), has the
    /// signing capability, etc.
    pub fn verify_userid_revocation<P, Q, R>(&self,
                                             signer: &Key<P, R>,
                                             pk: &Key<Q, key::PrimaryRole>,
                                             userid: &UserID)
        -> Result<()>
        where P: key::KeyParts,
              Q: key::KeyParts,
              R: key::KeyRole,
    {
        if self.typ() != SignatureType::CertificationRevocation {
            return Err(Error::UnsupportedSignatureType(self.typ()).into());
        }

        let hash = Signature::hash_userid_binding(self, pk, userid)?;
        self.verify_digest(signer, &hash[..])
    }

    /// Verifies the user attribute binding.
    ///
    /// `self` is the user attribute binding signature, `signer` is
    /// the key that allegedly made the signature, `pk` is the primary
    /// key, and `ua` is the user attribute.
    ///
    /// For a self-signature, `signer` and `pk` will be the same.
    ///
    /// Note: Due to limited context, this only verifies the
    /// cryptographic signature, checks the signature's type, and
    /// checks that the key predates the signature.  Further
    /// constraints on the signature, like creation and expiration
    /// time, or signature revocations must be checked by the caller.
    ///
    /// Likewise, this function does not check whether `signer` can
    /// made valid signatures; it is up to the caller to make sure the
    /// key is not revoked, not expired, has a valid self-signature,
    /// has a subkey binding signature (if appropriate), has the
    /// signing capability, etc.
    pub fn verify_user_attribute_binding<P, Q, R>(&self,
                                                  signer: &Key<P, R>,
                                                  pk: &Key<Q, key::PrimaryRole>,
                                                  ua: &UserAttribute)
        -> Result<()>
        where P: key::KeyParts,
              Q: key::KeyParts,
              R: key::KeyRole,
    {
        if !(self.typ() == SignatureType::GenericCertification
             || self.typ() == SignatureType::PersonaCertification
             || self.typ() == SignatureType::CasualCertification
             || self.typ() == SignatureType::PositiveCertification) {
            return Err(Error::UnsupportedSignatureType(self.typ()).into());
        }

        let hash = Signature::hash_user_attribute_binding(self, pk, ua)?;
        self.verify_digest(signer, &hash[..])
    }

    /// Verifies the user attribute revocation certificate.
    ///
    /// `self` is the user attribute binding signature, `signer` is
    /// the key that allegedly made the signature, `pk` is the primary
    /// key, and `ua` is the user attribute.
    ///
    /// For a self-signature, `signer` and `pk` will be the same.
    ///
    /// Note: Due to limited context, this only verifies the
    /// cryptographic signature, checks the signature's type, and
    /// checks that the key predates the signature.  Further
    /// constraints on the signature, like creation and expiration
    /// time, or signature revocations must be checked by the caller.
    ///
    /// Likewise, this function does not check whether `signer` can
    /// made valid signatures; it is up to the caller to make sure the
    /// key is not revoked, not expired, has a valid self-signature,
    /// has a subkey binding signature (if appropriate), has the
    /// signing capability, etc.
    pub fn verify_user_attribute_revocation<P, Q, R>(
        &self,
        signer: &Key<P, R>,
        pk: &Key<Q, key::PrimaryRole>,
        ua: &UserAttribute)
        -> Result<()>
        where P: key::KeyParts,
              Q: key::KeyParts,
              R: key::KeyRole,
    {
        if self.typ() != SignatureType::CertificationRevocation {
            return Err(Error::UnsupportedSignatureType(self.typ()).into());
        }

        let hash = Signature::hash_user_attribute_binding(self, pk, ua)?;
        self.verify_digest(signer, &hash[..])
    }

    /// Verifies a signature of a message.
    ///
    /// `self` is the message signature, `signer` is
    /// the key that allegedly made the signature and `msg` is the message.
    ///
    /// This function is for short messages, if you want to verify larger files
    /// use `Verifier`.
    ///
    /// Note: Due to limited context, this only verifies the
    /// cryptographic signature, checks the signature's type, and
    /// checks that the key predates the signature.  Further
    /// constraints on the signature, like creation and expiration
    /// time, or signature revocations must be checked by the caller.
    ///
    /// Likewise, this function does not check whether `signer` can
    /// made valid signatures; it is up to the caller to make sure the
    /// key is not revoked, not expired, has a valid self-signature,
    /// has a subkey binding signature (if appropriate), has the
    /// signing capability, etc.
    pub fn verify_message<M, P, R>(&self, signer: &Key<P, R>,
                                   msg: M)
        -> Result<()>
        where M: AsRef<[u8]>,
              P: key::KeyParts,
              R: key::KeyRole,
    {
        if self.typ() != SignatureType::Binary &&
            self.typ() != SignatureType::Text {
            return Err(Error::UnsupportedSignatureType(self.typ()).into());
        }

        // Compute the digest.
        let mut hash = self.hash_algo().context()?;
        let mut digest = vec![0u8; hash.digest_size()];

        hash.update(msg.as_ref());
        self.hash(&mut hash);
        hash.digest(&mut digest);

        self.verify_digest(signer, &digest[..])
    }
}

impl From<Signature4> for Packet {
    fn from(s: Signature4) -> Self {
        Packet::Signature(s.into())
    }
}

impl From<Signature4> for super::Signature {
    fn from(s: Signature4) -> Self {
        super::Signature::V4(s)
    }
}

#[cfg(any(test, feature = "quickcheck"))]
impl ArbitraryBounded for super::Signature {
    fn arbitrary_bounded<G: Gen>(g: &mut G, depth: usize) -> Self {
        Signature4::arbitrary_bounded(g, depth).into()
    }
}

#[cfg(any(test, feature = "quickcheck"))]
impl_arbitrary_with_bound!(super::Signature);

#[cfg(any(test, feature = "quickcheck"))]
impl ArbitraryBounded for Signature4 {
    fn arbitrary_bounded<G: Gen>(g: &mut G, depth: usize) -> Self {
        use mpi::MPI;
        use PublicKeyAlgorithm::*;

        let fields = SignatureFields::arbitrary_bounded(g, depth);
        #[allow(deprecated)]
        let mpis = match fields.pk_algo() {
            RSAEncryptSign | RSASign => mpi::Signature::RSA  {
                s: MPI::arbitrary(g),
            },

            DSA => mpi::Signature::DSA {
                r: MPI::arbitrary(g),
                s: MPI::arbitrary(g),
            },

            EdDSA => mpi::Signature::EdDSA  {
                r: MPI::arbitrary(g),
                s: MPI::arbitrary(g),
            },

            ECDSA => mpi::Signature::ECDSA  {
                r: MPI::arbitrary(g),
                s: MPI::arbitrary(g),
            },

            _ => unreachable!(),
        };

        Signature4 {
            common: Arbitrary::arbitrary(g),
            fields,
            digest_prefix: [Arbitrary::arbitrary(g),
                            Arbitrary::arbitrary(g)],
            mpis,
            computed_digest: None,
            level: 0,
        }
    }
}

#[cfg(any(test, feature = "quickcheck"))]
impl_arbitrary_with_bound!(Signature4);

#[cfg(test)]
mod test {
    use super::*;
    use crate::KeyID;
    use crate::cert::prelude::*;
    use crate::crypto;
    use crate::parse::Parse;
    use crate::packet::Key;
    use crate::packet::key::Key4;
    use crate::types::Curve;
    use crate::policy::StandardPolicy as P;

    #[cfg(feature = "compression-deflate")]
    #[test]
    fn signature_verification_test() {
        use super::*;

        use crate::Cert;
        use crate::parse::{PacketParserResult, PacketParser};

        struct Test<'a> {
            key: &'a str,
            data: &'a str,
            good: usize,
        };

        let tests = [
            Test {
                key: &"neal.pgp"[..],
                data: &"signed-1.gpg"[..],
                good: 1,
            },
            Test {
                key: &"neal.pgp"[..],
                data: &"signed-1-sha1-neal.gpg"[..],
                good: 1,
            },
            Test {
                key: &"testy.pgp"[..],
                data: &"signed-1-sha256-testy.gpg"[..],
                good: 1,
            },
            Test {
                key: &"dennis-simon-anton.pgp"[..],
                data: &"signed-1-dsa.pgp"[..],
                good: 1,
            },
            Test {
                key: &"erika-corinna-daniela-simone-antonia-nistp256.pgp"[..],
                data: &"signed-1-ecdsa-nistp256.pgp"[..],
                good: 1,
            },
            Test {
                key: &"erika-corinna-daniela-simone-antonia-nistp384.pgp"[..],
                data: &"signed-1-ecdsa-nistp384.pgp"[..],
                good: 1,
            },
            Test {
                key: &"erika-corinna-daniela-simone-antonia-nistp521.pgp"[..],
                data: &"signed-1-ecdsa-nistp521.pgp"[..],
                good: 1,
            },
            Test {
                key: &"emmelie-dorothea-dina-samantha-awina-ed25519.pgp"[..],
                data: &"signed-1-eddsa-ed25519.pgp"[..],
                good: 1,
            },
            Test {
                key: &"emmelie-dorothea-dina-samantha-awina-ed25519.pgp"[..],
                data: &"signed-twice-by-ed25519.pgp"[..],
                good: 2,
            },
            Test {
                key: "neal.pgp",
                data: "signed-1-notarized-by-ed25519.pgp",
                good: 1,
            },
            Test {
                key: "emmelie-dorothea-dina-samantha-awina-ed25519.pgp",
                data: "signed-1-notarized-by-ed25519.pgp",
                good: 1,
            },
            // Check with the wrong key.
            Test {
                key: &"neal.pgp"[..],
                data: &"signed-1-sha256-testy.gpg"[..],
                good: 0,
            },
            Test {
                key: &"neal.pgp"[..],
                data: &"signed-2-partial-body.gpg"[..],
                good: 1,
            },
        ];

        for test in tests.iter() {
            eprintln!("{}, expect {} good signatures:",
                      test.data, test.good);

            let cert = Cert::from_bytes(crate::tests::key(test.key)).unwrap();

            let mut good = 0;
            let mut ppr = PacketParser::from_bytes(
                crate::tests::message(test.data)).unwrap();
            while let PacketParserResult::Some(pp) = ppr {
                if let Packet::Signature(ref sig) = pp.packet {
                    let result = sig.verify(cert.primary_key().key())
                        .map(|_| true).unwrap_or(false);
                    eprintln!("  Primary {:?}: {:?}",
                              cert.fingerprint(), result);
                    if result {
                        good += 1;
                    }

                    for sk in cert.subkeys() {
                        let result = sig.verify(sk.key())
                            .map(|_| true).unwrap_or(false);
                        eprintln!("   Subkey {:?}: {:?}",
                                  sk.key().fingerprint(), result);
                        if result {
                            good += 1;
                        }
                    }
                }

                // Get the next packet.
                ppr = pp.recurse().unwrap().1;
            }

            assert_eq!(good, test.good, "Signature verification failed.");
        }
    }

    #[test]
    fn signature_level() {
        use crate::PacketPile;
        let p = PacketPile::from_bytes(
            crate::tests::message("signed-1-notarized-by-ed25519.pgp")).unwrap()
            .into_children().collect::<Vec<Packet>>();

        if let Packet::Signature(ref sig) = &p[3] {
            assert_eq!(sig.level(), 0);
        } else {
            panic!("expected signature")
        }

        if let Packet::Signature(ref sig) = &p[4] {
            assert_eq!(sig.level(), 1);
        } else {
            panic!("expected signature")
        }
    }

    #[test]
    fn sign_verify() {
        let hash_algo = HashAlgorithm::SHA512;
        let mut hash = vec![0; hash_algo.context().unwrap().digest_size()];
        crypto::random(&mut hash);

        for key in &[
            "testy-private.pgp",
            "dennis-simon-anton-private.pgp",
            "erika-corinna-daniela-simone-antonia-nistp256-private.pgp",
            "erika-corinna-daniela-simone-antonia-nistp384-private.pgp",
            "erika-corinna-daniela-simone-antonia-nistp521-private.pgp",
            "emmelie-dorothea-dina-samantha-awina-ed25519-private.pgp",
        ] {
            let cert = Cert::from_bytes(crate::tests::key(key)).unwrap();
            let mut pair = cert.primary_key().key().clone()
                .parts_into_secret().unwrap()
                .into_keypair()
                .expect("secret key is encrypted/missing");

            let sig = SignatureBuilder::new(SignatureType::Binary);
            let hash = hash_algo.context().unwrap();

            // Make signature.
            let sig = sig.sign_hash(&mut pair, hash).unwrap();

            // Good signature.
            let mut hash = hash_algo.context().unwrap();
            sig.hash(&mut hash);
            let mut digest = vec![0u8; hash.digest_size()];
            hash.digest(&mut digest);
            sig.verify_digest(pair.public(), &digest[..]).unwrap();

            // Bad signature.
            digest[0] ^= 0xff;
            sig.verify_digest(pair.public(), &digest[..]).unwrap_err();
        }
    }

    #[test]
    fn sign_message() {
        use crate::types::Curve;

        let key: Key<key::SecretParts, key::PrimaryRole>
            = Key4::generate_ecc(true, Curve::Ed25519)
            .unwrap().into();
        let msg = b"Hello, World";
        let mut pair = key.into_keypair().unwrap();
        let sig = SignatureBuilder::new(SignatureType::Binary)
            .sign_message(&mut pair, msg).unwrap();

        sig.verify_message(pair.public(), msg).unwrap();
    }

    #[test]
    fn verify_message() {
        let cert = Cert::from_bytes(crate::tests::key(
                "emmelie-dorothea-dina-samantha-awina-ed25519.pgp")).unwrap();
        let msg = crate::tests::manifesto();
        let p = Packet::from_bytes(
            crate::tests::message("a-cypherpunks-manifesto.txt.ed25519.sig"))
            .unwrap();
        let sig = if let Packet::Signature(s) = p {
            s
        } else {
            panic!("Expected a Signature, got: {:?}", p);
        };

        sig.verify_message(cert.primary_key().key(), &msg[..]).unwrap();
    }

    #[test]
    fn sign_with_short_ed25519_secret_key() {
        // 20 byte sec key
        let secret_key = [
            0x0,0x0,
            0x0,0x0,0x0,0x0,0x0,0x0,0x0,0x0,0x0,0x0,
            0x1,0x2,0x2,0x2,0x2,0x2,0x2,0x2,0x2,0x2,
            0x1,0x2,0x2,0x2,0x2,0x2,0x2,0x2,0x2,0x2
        ];

        let key: key::SecretKey = Key4::import_secret_ed25519(&secret_key, None)
            .unwrap().into();

        let mut pair = key.into_keypair().unwrap();
        let msg = b"Hello, World";
        let mut hash = HashAlgorithm::SHA256.context().unwrap();

        hash.update(&msg[..]);

        SignatureBuilder::new(SignatureType::Text)
            .sign_hash(&mut pair, hash).unwrap();
    }

    #[test]
    fn verify_gpg_3rd_party_cert() {
        use crate::Cert;

        let p = &P::new();

        let test1 = Cert::from_bytes(
            crate::tests::key("test1-certification-key.pgp")).unwrap();
        let cert_key1 = test1.keys().with_policy(p, None)
            .for_certification()
            .nth(0)
            .map(|ka| ka.key())
            .unwrap();
        let test2 = Cert::from_bytes(
            crate::tests::key("test2-signed-by-test1.pgp")).unwrap();
        let uid = test2.userids().with_policy(p, None).nth(0).unwrap();
        let cert = &uid.certifications()[0];

        cert.verify_userid_binding(cert_key1,
                                   test2.primary_key().key(),
                                   uid.userid()).unwrap();
    }

    #[test]
    fn normalize() {
        use crate::Fingerprint;
        use crate::packet::signature::subpacket::*;

        let key : key::SecretKey
            = Key4::generate_ecc(true, Curve::Ed25519).unwrap().into();
        let mut pair = key.into_keypair().unwrap();
        let msg = b"Hello, World";
        let mut hash = HashAlgorithm::SHA256.context().unwrap();
        hash.update(&msg[..]);

        let fp = Fingerprint::from_bytes(b"bbbbbbbbbbbbbbbbbbbb");
        let keyid = KeyID::from(&fp);

        // First, make sure any superfluous subpackets are removed,
        // yet the Issuer, IssuerFingerprint and EmbeddedSignature
        // ones are kept.
        let mut builder = SignatureBuilder::new(SignatureType::Text);
        builder.unhashed_area_mut().add(Subpacket::new(
            SubpacketValue::IssuerFingerprint(fp.clone()), false).unwrap())
            .unwrap();
        builder.unhashed_area_mut().add(Subpacket::new(
            SubpacketValue::Issuer(keyid.clone()), false).unwrap())
            .unwrap();
        // This subpacket does not belong there, and should be
        // removed.
        builder.unhashed_area_mut().add(Subpacket::new(
            SubpacketValue::PreferredSymmetricAlgorithms(Vec::new()),
            false).unwrap()).unwrap();

        // Build and add an embedded sig.
        let embedded_sig = SignatureBuilder::new(SignatureType::PrimaryKeyBinding)
            .sign_hash(&mut pair, hash.clone()).unwrap();
        builder.unhashed_area_mut().add(Subpacket::new(
            SubpacketValue::EmbeddedSignature(embedded_sig.into()), false)
                                        .unwrap()).unwrap();
        let sig = builder.sign_hash(&mut pair,
                                    hash.clone()).unwrap().normalize();
        assert_eq!(sig.unhashed_area().iter().count(), 3);
        assert_eq!(*sig.unhashed_area().iter().nth(0).unwrap(),
                   Subpacket::new(SubpacketValue::Issuer(keyid.clone()),
                                  false).unwrap());
        assert_eq!(sig.unhashed_area().iter().nth(1).unwrap().tag(),
                   SubpacketTag::EmbeddedSignature);
        assert_eq!(*sig.unhashed_area().iter().nth(2).unwrap(),
                   Subpacket::new(SubpacketValue::IssuerFingerprint(fp.clone()),
                                  false).unwrap());
    }

    #[test]
    fn standalone_signature_roundtrip() {
        let key : key::SecretKey
            = Key4::generate_ecc(true, Curve::Ed25519).unwrap().into();
        let mut pair = key.into_keypair().unwrap();

        let sig = SignatureBuilder::new(SignatureType::Standalone)
            .sign_standalone(&mut pair)
            .unwrap();

        sig.verify_standalone(pair.public()).unwrap();
    }

    #[test]
    fn timestamp_signature() {
        let alpha = Cert::from_bytes(crate::tests::file(
            "contrib/gnupg/keys/alpha.pgp")).unwrap();
        let p = Packet::from_bytes(crate::tests::file(
            "contrib/gnupg/timestamp-signature-by-alice.asc")).unwrap();
        if let Packet::Signature(sig) = p {
            let digest = Signature::hash_standalone(&sig).unwrap();
            eprintln!("{}", crate::fmt::hex::encode(&digest));
            sig.verify_timestamp(alpha.primary_key().key()).unwrap();
        } else {
            panic!("expected a signature packet");
        }
    }

    #[test]
    fn timestamp_signature_roundtrip() {
        let key : key::SecretKey
            = Key4::generate_ecc(true, Curve::Ed25519).unwrap().into();
        let mut pair = key.into_keypair().unwrap();

        let sig = SignatureBuilder::new(SignatureType::Timestamp)
            .sign_timestamp(&mut pair)
            .unwrap();

        sig.verify_timestamp(pair.public()).unwrap();
    }

    #[test]
    fn get_issuers_prefers_fingerprints() -> Result<()> {
        use crate::KeyHandle;
        for f in [
            // This has Fingerprint in the hashed, Issuer in the
            // unhashed area.
            "messages/sig.gpg",
            // This has [Issuer, Fingerprint] in the hashed area.
            "contrib/gnupg/timestamp-signature-by-alice.asc",
        ].iter() {
            let p = Packet::from_bytes(crate::tests::file(f))?;
            if let Packet::Signature(sig) = p {
                let issuers = sig.get_issuers();
                assert_match!(KeyHandle::Fingerprint(_) = &issuers[0]);
                assert_match!(KeyHandle::KeyID(_) = &issuers[1]);
            } else {
                panic!("expected a signature packet");
            }
        }
        Ok(())
    }
}
