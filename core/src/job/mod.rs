use crate::{
	library::LibraryContext,
	location::{indexer::IndexerError, LocationError},
	object::{identifier_job::IdentifierJobError, preview::ThumbnailError},
};

use std::{collections::VecDeque, fmt::Debug, hash::Hash, sync::Arc};

use rmp_serde::{decode::Error as DecodeError, encode::Error as EncodeError};
use sd_crypto::Error as CryptoError;
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use thiserror::Error;
use uuid::Uuid;

mod job_manager;
mod job_report;
mod worker;

pub use job_manager::*;
pub use job_report::*;
pub use worker::*;

/// TODO
#[derive(Error, Debug)]
pub enum JobError {
	// General errors
	#[error("Database error: {0}")]
	DatabaseError(#[from] prisma_client_rust::QueryError),
	#[error("I/O error: {0}")]
	IOError(#[from] std::io::Error),
	#[error("Failed to join Tokio spawn blocking: {0}")]
	JoinTaskError(#[from] tokio::task::JoinError),
	#[error("Job state encode error: {0}")]
	StateEncode(#[from] EncodeError),
	#[error("Job state decode error: {0}")]
	StateDecode(#[from] DecodeError),
	#[error("Job metadata serialization error: {0}")]
	MetadataSerialization(#[from] serde_json::Error),
	#[error("Tried to resume a job with unknown name: job <name='{1}', uuid='{0}'>")]
	UnknownJobName(Uuid, String),
	#[error(
		"Tried to resume a job that doesn't have saved state data: job <name='{1}', uuid='{0}'>"
	)]
	MissingJobDataState(Uuid, String),

	// Specific job errors
	#[error("Indexer error: {0}")]
	IndexerError(#[from] IndexerError),
	#[error("Location error: {0}")]
	LocationError(#[from] LocationError),
	#[error("Thumbnail error: {0}")]
	ThumbnailError(#[from] ThumbnailError),
	#[error("Identifier error: {0}")]
	IdentifierError(#[from] IdentifierJobError),

	// Not errors
	#[error("Job had a early finish: '{0}'")]
	EarlyFinish(/* Reason */ String),
	#[error("Crypto error: {0}")]
	CryptoError(#[from] CryptoError),
	#[error("Data needed for job execution not found: job <name='{0}'>")]
	JobDataNotFound(String),
	#[error("Job paused")]
	Paused(Vec<u8>),
}

/// TODO
pub type JobResult = Result<JobMetadata, JobError>;

/// TODO
pub type JobMetadata = Option<serde_json::Value>;

/// TODO
pub trait JobInitData {
	type Job: StatefulJob;
}

/// TODO
#[async_trait::async_trait]
pub trait StatefulJob: Send + Sync + Sized + JobRestorer + 'static {
	/// TODO
	type Init: Serialize + DeserializeOwned + Send + Sync + Hash + JobInitData<Job = Self>;
	/// TODO
	type Data: Serialize + DeserializeOwned + Send + Sync;
	/// TODO
	type Step: Serialize + DeserializeOwned + Send + Sync;

	/// The name of the job is a unique human readable identifier for the job.
	const NAME: &'static str;

	fn new() -> Self;

	/// TODO
	async fn init(
		&self,
		ctx: &mut WorkerContext,
		state: &mut JobState<Self>,
	) -> Result<(), JobError>;

	/// TODO
	async fn execute_step(
		&self,
		ctx: &mut WorkerContext,
		state: &mut JobState<Self>,
	) -> Result<(), JobError>;

	/// TODO
	async fn finalize(&self, ctx: &mut WorkerContext, state: &mut JobState<Self>) -> JobResult;
}

/// TODO
#[async_trait::async_trait]
pub trait JobRestorer {
	async fn restore(
		&self,
		job_manager: Arc<JobManager>,
		ctx: &LibraryContext,
		report: JobReport,
		job_state_data: Vec<u8>,
	) -> Result<(), JobError>;
}

#[async_trait::async_trait]
impl<T: StatefulJob> JobRestorer for T {
	async fn restore(
		&self,
		job_manager: Arc<JobManager>,
		ctx: &LibraryContext,
		report: JobReport,
		job_state_data: Vec<u8>,
	) -> Result<(), JobError> {
		job_manager
			.internal_dispatch_job(
				ctx.clone(),
				report,
				rmp_serde::from_slice(&job_state_data)?,
				Self::new(),
			)
			.await;
		Ok(())
	}
}

/// TODO
#[derive(Serialize, Deserialize)]
pub struct JobState<Job: StatefulJob> {
	pub init: Job::Init,
	pub data: Option<Job::Data>,
	pub steps: VecDeque<Job::Step>,
	pub step_number: usize,
}
