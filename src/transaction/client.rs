use reqwest::{
    header::{ACCEPT, CONTENT_TYPE},
    StatusCode,
};
use std::{str::FromStr, thread::sleep, time::Duration};

use crate::{
    consts::{ARWEAVE_BASE_URL, CHUNKS_RETRIES, CHUNKS_RETRY_SLEEP},
    crypto::base64::Base64,
    error::Error,
    types::TxStatus,
};

use super::Tx;

pub struct TxClient {
    client: reqwest::Client,
    base_url: url::Url,
}

impl Default for TxClient {
    fn default() -> Self {
        Self {
            client: reqwest::Client::new(),
            base_url: url::Url::from_str(ARWEAVE_BASE_URL).unwrap(),
        }
    }
}

impl TxClient {
    pub fn new(client: reqwest::Client, base_url: url::Url) -> Result<Self, Error> {
        Ok(Self { client, base_url })
    }

    pub async fn post_transaction(&self, signed_transaction: &Tx) -> Result<(Base64, u64), Error> {
        if signed_transaction.id.0.is_empty() {
            return Err(Error::UnsignedTransaction);
        }

        let mut retries = 0;
        let mut status = reqwest::StatusCode::NOT_FOUND;
        let url = self
            .base_url
            .join("tx")
            .expect("Could not join base_url with /tx");
        while (retries < CHUNKS_RETRIES) & (status != reqwest::StatusCode::OK) {
            let res = self
                .client
                .post(url.clone())
                .json(&signed_transaction)
                .header(&ACCEPT, "application/json")
                .header(&CONTENT_TYPE, "application/json")
                .send()
                .await
                .expect("Could not post transaction");
            status = res.status();
            dbg!(status);
            if status == reqwest::StatusCode::OK {
                return Ok((signed_transaction.id.clone(), signed_transaction.reward));
            }
            sleep(Duration::from_secs(CHUNKS_RETRY_SLEEP));
            retries += 1;
        }

        Err(Error::StatusCodeNotOk)
    }

    pub async fn get_last_tx(&self) -> Base64 {
        let resp = self
            .client
            .get(
                self.base_url
                    .join("tx_anchor")
                    .expect("Could not join base_url with /tx_anchor"),
            )
            .send()
            .await
            .expect("Could not get last tx");
        let last_tx_str = resp.text().await.unwrap();
        Base64::from_str(&last_tx_str).unwrap()
    }

    pub async fn get_fee(&self, target: &Base64, data: &[u8]) -> Result<u64, Error> {
        let url = self
            .base_url
            .join(&format!("price/{}/{}", data.len(), target))
            .expect("Could not join base_url with /price/{}/{}");
        let winstons_per_bytes = reqwest::get(url)
            .await
            .map_err(|e| Error::GetPriceError(e.to_string()))?
            .json::<u64>()
            .await
            .expect("Could not get base fee");
        Ok(winstons_per_bytes)
    }

    pub async fn get_fee_by_filesize(&self, size: u64) -> Result<u64, Error> {
        let url = self
            .base_url
            .join(&format!("price/{}", size))
            .expect("Could not join base_url with /price/{}/{}");
        let winstons_per_bytes = reqwest::get(url)
            .await
            .map_err(|e| Error::GetPriceError(e.to_string()))?
            .json::<u64>()
            .await
            .expect("Could not get base fee");
        Ok(winstons_per_bytes)
    }

    pub async fn get_tx(&self, id: &Base64) -> Result<(StatusCode, Option<Tx>), Error> {
        let res = self
            .client
            .get(
                self.base_url
                    .join(&format!("tx/{}", id))
                    .expect("Could not join base_url with /tx"),
            )
            .send()
            .await
            .expect("Could not get tx");

        if res.status() == StatusCode::OK {
            let text = res
                .text()
                .await
                .expect("Could not parse response to string");
            let tx = Tx::from_str(&text).expect("Could not create Tx from string");
            return Ok((StatusCode::OK, Some(tx)));
        } else if res.status() == StatusCode::ACCEPTED {
            //Tx is pending
            return Ok((StatusCode::ACCEPTED, None));
        }

        Err(Error::TransactionInfoError(res.status().to_string()))
    }

    pub async fn get_tx_data(&self, id: &Base64) -> Result<(StatusCode, Option<Vec<u8>>), Error> {
        let res = self
            .client
            .get(
                self.base_url
                    .join(&format!("tx/{}/data", id))
                    .expect("Could not join base_url with /tx"),
            )
            .send()
            .await
            .expect("Could not get tx");

        if res.status() == StatusCode::OK {
            let text = res
                .text()
                .await
                .expect("Could not parse response to string");
            let body = Base64::from_str(text.as_str()).expect("fail to decode body");
            return Ok((StatusCode::OK, Some(body.0)));
        } else if res.status() == StatusCode::ACCEPTED {
            //Tx is pending
            return Ok((StatusCode::ACCEPTED, None));
        }

        Err(Error::TransactionInfoError(res.status().to_string()))
    }

    pub async fn get_tx_status(
        &self,
        id: &Base64,
    ) -> Result<(StatusCode, Option<TxStatus>), Error> {
        let res = self
            .client
            .get(
                self.base_url
                    .join(&format!("tx/{}/status", id))
                    .expect("Could not join base_url with /tx/{}/status"),
            )
            .send()
            .await
            .expect("Could not get tx status");

        if res.status() == StatusCode::OK {
            let status = res
                .json::<TxStatus>()
                .await
                .map_err(|err| Error::TransactionInfoError(err.to_string()))
                .expect("Could not parse tx status");

            Ok((StatusCode::OK, Some(status)))
        } else if res.status() == StatusCode::ACCEPTED {
            Ok((StatusCode::ACCEPTED, None))
        } else {
            Err(Error::TransactionInfoError(res.status().to_string()))
        }
    }
}
