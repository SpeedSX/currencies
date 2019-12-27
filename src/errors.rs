use serde::Serialize;
use thiserror::Error;
use warp::http::StatusCode;
use warp::{Rejection, Reply};

#[derive(Serialize)]
struct ErrorMessage {
    code: u16,
    msg: String,
}

pub async fn recover(err: Rejection) -> Result<Box<dyn Reply>, Rejection> {
    //api errors should be returned in json
    if let Some(ref err) = err.find::<Error>() {
        let error = match err {
            Error::InvalidDateFormat(_, _)
            | Error::PastDate(_)
            | Error::InvalidSymbol
            | Error::MissingDateBoundaries
            | Error::InvalidDateRange
            | Error::InvalidBase(_) => {
                log::trace!("api reject, {}", err);
                ErrorMessage {
                    code: StatusCode::BAD_REQUEST.as_u16(),
                    msg: err.to_string(),
                }
            }
            Error::DateNotFound(_) => {
                log::trace!("api reject, {}", err);
                ErrorMessage {
                    code: StatusCode::NOT_FOUND.as_u16(),
                    msg: "Not Found".into(),
                }
            }
            _ => {
                log::error!("unhandled error! {}", err);
                ErrorMessage {
                    code: StatusCode::INTERNAL_SERVER_ERROR.as_u16(),
                    msg: "Internal Server Error".into(),
                }
            }
        };

        // cast needed for now:
        // https://github.com/rust-lang/rust/pull/64999#issuecomment-540622253
        return Ok(Box::new(warp::reply::with_status(
            warp::reply::json(&error),
            StatusCode::from_u16(error.code).unwrap(),
        )) as Box<dyn warp::Reply>);
    };

    Ok(Box::new(StatusCode::INTERNAL_SERVER_ERROR) as Box<dyn warp::Reply>)
}

#[derive(Error, Debug)]
pub enum Error {
    #[error("no curencies found for date `{0}`")]
    DateNotFound(String),
    #[error("could not parse `{0}` as NaiveDate")]
    DateParseError(String),
    #[error("`{0}` is invalid, there are no currency rates for dates older then 1999-01-04.")]
    PastDate(&'static str),
    #[error("`{0}` is an invalid port")]
    InvalidPort(String),
    #[error("start_at must be older than end_at")]
    InvalidDateRange,
    #[error("`{0}`: `{1}` is in an invalid date format, date must be in the format %Y-%m-%d")]
    InvalidDateFormat(&'static str, String),
    #[error("`{0}` is an invalid base currency")]
    InvalidBase(String),
    #[error("empty currency dataset, should have at least 1 element")]
    EmpyDataset,
    #[error("symbol list contains invalid symbols")]
    InvalidSymbol,
    #[error("both start_at and end_at parameters must be present")]
    MissingDateBoundaries,
    #[error("database error, `{0}`")]
    DatabaseError(String),
    #[error("error fetching currencies from ECB, `{0}`")]
    FetcherError(String),
    #[error("error rendering template, `{0}`")]
    TemplateError(String),
}

impl warp::reject::Reject for Error {}

impl From<Error> for warp::reject::Rejection {
    fn from(error: Error) -> warp::reject::Rejection {
        warp::reject::custom(error)
    }
}
