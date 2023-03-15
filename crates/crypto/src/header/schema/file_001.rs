use crate::{
	crypto::{Decryptor, Encryptor},
	header::file::{Header, HeaderObjectType},
	primitives::FILE_KEY_CONTEXT,
	types::{Aad, Algorithm, EncryptedKey, HashingAlgorithm, Key, Nonce, Salt},
	Error, Protected, Result,
};

const KEYSLOT_LIMIT: usize = 2;
const OBJECT_LIMIT: usize = 2;

#[derive(Clone, bincode::Encode, bincode::Decode)]
pub struct FileHeader001 {
	pub aad: Aad,
	pub algorithm: Algorithm,
	pub nonce: Nonce,
	pub keyslots: Vec<Keyslot001>,
	pub objects: Vec<FileHeaderObject001>,
}

/// A keyslot - 96 bytes (as of V1), and contains all the information for future-proofing while keeping the size reasonable
#[derive(bincode::Encode, bincode::Decode, Clone)]
pub struct Keyslot001 {
	pub hashing_algorithm: HashingAlgorithm, // password hashing algorithm
	pub salt: Salt, // the salt used for deriving a KEK from a (key/content salt) hash
	pub content_salt: Salt,
	pub master_key: EncryptedKey, // encrypted
	pub nonce: Nonce,
}

#[derive(Clone, bincode::Encode, bincode::Decode)]
pub struct FileHeaderObject001 {
	pub object_type: HeaderObjectType,
	pub nonce: Nonce,
	pub data: Vec<u8>,
}

impl Keyslot001 {
	#[allow(clippy::needless_pass_by_value)]
	pub async fn new(
		algorithm: Algorithm,
		hashing_algorithm: HashingAlgorithm,
		content_salt: Salt,
		hashed_key: Key,
		master_key: Key,
	) -> Result<Self> {
		let nonce = Nonce::generate(algorithm)?;

		let salt = Salt::generate();

		let encrypted_master_key = EncryptedKey::try_from(
			Encryptor::encrypt_bytes(
				Key::derive(hashed_key, salt, FILE_KEY_CONTEXT),
				nonce,
				algorithm,
				master_key.expose(),
				&[],
			)
			.await?,
		)?;

		Ok(Self {
			hashing_algorithm,
			salt,
			content_salt,
			master_key: encrypted_master_key,
			nonce,
		})
	}

	#[allow(clippy::needless_pass_by_value)]
	async fn decrypt(&self, algorithm: Algorithm, key: Key) -> Result<Key> {
		Key::try_from(
			Decryptor::decrypt_bytes(
				Key::derive(key, self.salt, FILE_KEY_CONTEXT),
				self.nonce,
				algorithm,
				&self.master_key,
				&[],
			)
			.await?,
		)
	}
}

impl FileHeader001 {
	// TODO(brxken128): make the AAD not static
	// should be brought in from the raw file bytes but bincode makes that harder
	// as the first 32~ bytes of the file *may* change
	pub fn new(algorithm: Algorithm) -> Result<Self> {
		let f = Self {
			aad: Aad::generate(),
			algorithm,
			nonce: Nonce::generate(algorithm)?,
			keyslots: vec![],
			objects: vec![],
		};

		Ok(f)
	}
}

impl FileHeaderObject001 {
	pub async fn new(
		object_type: HeaderObjectType,
		algorithm: Algorithm,
		master_key: Key,
		aad: Aad,
		data: &[u8],
	) -> Result<Self> {
		let nonce = Nonce::generate(algorithm)?;

		let encrypted_data =
			Encryptor::encrypt_bytes(master_key, nonce, algorithm, data, &aad).await?;

		let object = Self {
			object_type,
			nonce,
			data: encrypted_data,
		};

		Ok(object)
	}

	async fn decrypt(
		&self,
		algorithm: Algorithm,
		aad: Aad,
		master_key: Key,
	) -> Result<Protected<Vec<u8>>> {
		let pvm =
			Decryptor::decrypt_bytes(master_key, self.nonce, algorithm, &self.data, &aad).await?;

		Ok(pvm)
	}
}

#[async_trait::async_trait]
impl Header for FileHeader001 {
	fn serialize(&self) -> Result<Vec<u8>> {
		bincode::encode_to_vec(self, bincode::config::standard()).map_err(Error::BincodeEncode)
	}

	async fn decrypt_object(&self, index: usize, master_key: Key) -> Result<Protected<Vec<u8>>> {
		if index >= self.objects.len() || self.objects.is_empty() {
			return Err(Error::Index);
		}

		self.objects[index]
			.decrypt(self.algorithm, self.aad, master_key)
			.await
	}

	async fn add_keyslot(
		&mut self,
		hashing_algorithm: HashingAlgorithm,
		content_salt: Salt,
		hashed_key: Key,
		master_key: Key,
	) -> Result<()> {
		if self.keyslots.len() + 1 > KEYSLOT_LIMIT {
			return Err(Error::TooManyKeyslots);
		}

		self.keyslots.push(
			Keyslot001::new(
				self.algorithm,
				hashing_algorithm,
				content_salt,
				hashed_key,
				master_key,
			)
			.await?,
		);
		Ok(())
	}

	async fn add_object(
		&mut self,
		object_type: HeaderObjectType,
		master_key: Key,
		data: &[u8],
	) -> Result<()> {
		if self.objects.len() + 1 > OBJECT_LIMIT {
			return Err(Error::TooManyObjects);
		}

		self.objects.push(
			FileHeaderObject001::new(object_type, self.algorithm, master_key, self.aad, data)
				.await?,
		);
		Ok(())
	}

	#[allow(clippy::needless_pass_by_value)]
	async fn decrypt_master_key(&self, keys: Vec<Key>) -> Result<Key> {
		if self.keyslots.is_empty() {
			return Err(Error::NoKeyslots);
		}

		for hashed_key in keys {
			for v in &self.keyslots {
				if let Ok(key) = v.decrypt(self.algorithm, hashed_key.clone()).await {
					return Ok(key);
				}
			}
		}

		Err(Error::IncorrectPassword)
	}

	#[allow(clippy::needless_pass_by_value)]
	async fn decrypt_master_key_with_password(&self, password: Protected<Vec<u8>>) -> Result<Key> {
		if self.keyslots.is_empty() {
			return Err(Error::NoKeyslots);
		}

		for v in &self.keyslots {
			let key = v
				.hashing_algorithm
				.hash(password.clone(), v.content_salt, None)
				.map_err(|_| Error::PasswordHash)?;

			if let Ok(key) = v.decrypt(self.algorithm, key).await {
				return Ok(key);
			}
		}

		Err(Error::IncorrectPassword)
	}

	fn get_aad(&self) -> Aad {
		self.aad
	}

	fn get_nonce(&self) -> Nonce {
		self.nonce
	}

	fn get_algorithm(&self) -> Algorithm {
		self.algorithm
	}

	fn count_objects(&self) -> usize {
		self.objects.len()
	}

	fn count_keyslots(&self) -> usize {
		self.keyslots.len()
	}
}