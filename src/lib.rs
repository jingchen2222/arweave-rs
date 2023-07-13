use consts::{ARWEAVE_BASE_URL, MAX_TX_DATA};
use crypto::base64::Base64;
use error::Error;
use futures::{stream, Stream, StreamExt};
use pretend::StatusCode;
use reqwest::Client;
use rsa::RsaPrivateKey;
use serde::{Deserialize, Serialize};
use std::str::FromStr;
use std::{fs, path::Path};
use transaction::{
    client::TxClient,
    tags::{FromUtf8Strs, Tag},
    Tx,
};
use types::TxStatus;
use upload::Uploader;

pub mod client;
pub mod consts;
pub mod crypto;
pub mod currency;
pub mod error;
pub mod network;
pub mod signer;
pub mod transaction;
pub mod types;
pub mod upload;
pub mod wallet;

pub use signer::ArweaveSigner;

#[derive(Serialize, Deserialize, Debug)]
pub struct OraclePrice {
    pub arweave: OraclePricePair,
}

#[derive(Serialize, Deserialize, Debug)]
pub struct OraclePricePair {
    pub usd: f32,
}

pub struct Arweave {
    pub base_url: url::Url,
    pub signer: ArweaveSigner,
    tx_client: TxClient,
    uploader: Uploader,
}

impl Default for Arweave {
    fn default() -> Self {
        let arweave_url = url::Url::from_str(ARWEAVE_BASE_URL).unwrap();
        Self {
            base_url: arweave_url,
            signer: Default::default(),
            tx_client: TxClient::default(),
            uploader: Default::default(),
        }
    }
}

impl Arweave {
    pub fn from_private_key(priv_key: RsaPrivateKey, base_url: url::Url) -> Result<Arweave, Error> {
        let tx_client = TxClient::new(reqwest::Client::new(), base_url.clone())
            .expect("Could not create TxClient");
        let signer = ArweaveSigner::from_private_key(priv_key).expect("Could not create TxClient");
        let uploader = Uploader::new(base_url.clone());
        let arweave = Arweave {
            base_url,
            signer,
            tx_client,
            uploader,
        };
        Ok(arweave)
    }

    pub fn from_keypair_path(keypair_path: &Path, base_url: url::Url) -> Result<Arweave, Error> {
        let signer =
            ArweaveSigner::from_keypair_path(keypair_path).expect("Could not create signer");
        let tx_client = TxClient::new(reqwest::Client::new(), base_url.clone())
            .expect("Could not create TxClient");
        let uploader = Uploader::new(base_url.clone());
        let arweave = Arweave {
            base_url,
            signer,
            tx_client,
            uploader,
        };
        Ok(arweave)
    }

    pub async fn create_transaction(
        &self,
        target: Base64,
        other_tags: Vec<Tag<Base64>>,
        data: Vec<u8>,
        quantity: u128,
        fee: u64,
        auto_content_tag: bool,
    ) -> Result<Tx, Error> {
        let last_tx = self.get_last_tx().await;
        Tx::new(
            self.signer.get_provider(),
            target,
            data,
            quantity,
            fee,
            last_tx,
            other_tags,
            auto_content_tag,
        )
    }

    pub fn sign_transaction(&self, transaction: Tx) -> Result<Tx, Error> {
        self.signer.sign_transaction(transaction)
    }

    pub fn sign(&self, message: &[u8]) -> Vec<u8> {
        self.signer.sign(message).0
    }

    pub fn verify_transaction(&self, transaction: &Tx) -> Result<(), Error> {
        ArweaveSigner::verify_transaction(transaction)
    }

    pub fn verify(pub_key: &[u8], message: &[u8], signature: &[u8]) -> Result<(), Error> {
        ArweaveSigner::verify(pub_key, message, signature)
    }

    pub async fn post_transaction(&self, signed_transaction: &Tx) -> Result<(String, u64), Error> {
        self.tx_client
            .post_transaction(signed_transaction)
            .await
            .map(|(id, reward)| (id.to_string(), reward))
    }

    async fn get_last_tx(&self) -> Base64 {
        self.tx_client.get_last_tx().await
    }

    pub async fn get_fee(&self, target: &Base64, data: &[u8]) -> Result<u64, Error> {
        self.tx_client.get_fee(target, data).await
    }

    pub async fn get_fee_by_size(&self, size: u64) -> Result<u64, Error> {
        self.tx_client.get_fee_by_filesize(size).await
    }

    pub async fn get_tx(&self, id: &Base64) -> Result<(StatusCode, Option<Tx>), Error> {
        self.tx_client.get_tx(id).await
    }

    pub async fn get_tx_data(&self, id: &Base64) -> Result<(StatusCode, Option<Vec<u8>>), Error> {
        self.tx_client.get_tx_data(id).await
    }

    pub async fn get_tx_status(
        &self,
        id: &Base64,
    ) -> Result<(StatusCode, Option<TxStatus>), Error> {
        self.tx_client.get_tx_status(id).await
    }

    pub fn get_pub_key(&self) -> String {
        self.signer.keypair_modulus().to_string()
    }

    pub fn get_wallet_address(&self) -> String {
        self.signer.wallet_address().to_string()
    }

    pub async fn upload_file_from_path(
        &self,
        file_path: &Path,
        additional_tags: Vec<Tag<Base64>>,
        fee: u64,
    ) -> Result<(String, u64), Error> {
        let mut auto_content_tag = true;
        let mut additional_tags = additional_tags;

        if let Some(content_type) = mime_guess::from_path(file_path).first() {
            auto_content_tag = false;
            let content_tag: Tag<Base64> =
                Tag::from_utf8_strs("Content-Type", content_type.as_ref())?;
            additional_tags.push(content_tag);
        }

        let data = fs::read(file_path).expect("Could not read file");
        let transaction = self
            .create_transaction(
                Base64(b"".to_vec()),
                additional_tags,
                data,
                0,
                fee,
                auto_content_tag,
            )
            .await
            .expect("Could not create transaction");
        let signed_transaction = self
            .sign_transaction(transaction)
            .expect("Could not sign tx");
        let (id, reward) = if signed_transaction.data.0.len() > MAX_TX_DATA as usize {
            self.post_transaction_chunks(signed_transaction, 100)
                .await
                .expect("Could not post transaction chunks")
        } else {
            self.post_transaction(&signed_transaction)
                .await
                .expect("Could not post transaction")
        };

        Ok((id, reward))
    }

    async fn post_transaction_chunks(
        &self,
        signed_transaction: Tx,
        chunks_buffer: usize,
    ) -> Result<(String, u64), Error> {
        if signed_transaction.id.0.is_empty() {
            return Err(error::Error::UnsignedTransaction);
        }

        let transaction_with_no_data = signed_transaction.clone_with_no_data()?;
        let (id, reward) = self.post_transaction(&transaction_with_no_data).await?;

        let results: Vec<Result<usize, Error>> =
            Self::upload_transaction_chunks_stream(self, signed_transaction, chunks_buffer)
                .collect()
                .await;

        results.into_iter().collect::<Result<Vec<usize>, Error>>()?;

        Ok((id, reward))
    }

    fn upload_transaction_chunks_stream(
        arweave: &Arweave,
        signed_transaction: Tx,
        buffer: usize,
    ) -> impl Stream<Item = Result<usize, Error>> + '_ {
        let client = Client::new();
        stream::iter(0..signed_transaction.chunks.len())
            .map(move |i| {
                let chunk = signed_transaction.get_chunk(i).unwrap();
                arweave
                    .uploader
                    .post_chunk_with_retries(chunk, client.clone())
            })
            .buffer_unordered(buffer)
    }
}

#[cfg(test)]
mod tests {
    use std::{fs::File, io::Read, path::PathBuf, str::FromStr};

    use pretend::Url;

    use crate::{error::Error, transaction::Tx, Arweave, ARWEAVE_BASE_URL};

    #[test]
    pub fn should_parse_and_verify_valid_tx() -> Result<(), Error> {
        let mut file = File::open("res/sample_tx.json").unwrap();
        let mut data = String::new();
        file.read_to_string(&mut data).unwrap();
        let tx = Tx::from_str(&data).unwrap();

        let path = PathBuf::from_str("res/test_wallet.json").unwrap();
        let arweave =
            Arweave::from_keypair_path(path.as_path(), Url::from_str(ARWEAVE_BASE_URL).unwrap())
                .unwrap();

        match arweave.verify_transaction(&tx) {
            Ok(_) => Ok(()),
            Err(_) => Err(Error::InvalidSignature),
        }
    }
}
