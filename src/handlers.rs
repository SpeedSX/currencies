use std::sync::Arc;
use std::cmp::Ordering;

use askama::Template;

use crate::currencies::Currency;
use crate::db::Db;

#[derive(Template)]
#[template(path = "index.html")]
struct CurrenciesTemplate<'a> {
    date: &'a str,
    currencies: &'a [Currency],
}

pub async fn index(db: Arc<Db>) -> Result<impl warp::Reply, warp::Rejection> {
    let mut date = db
        .get_current_rates()
        .await
        .map_err(|e| warp::reject::custom(e))?;

    // order currencies so that EUR comes first then gomes USD and then GBP
    date.currencies.sort_by(|curr1, curr2| {
        match (curr1.name.as_ref(), curr2.name.as_ref()) {
            ("EUR", _) => Ordering::Less,
            (_, "EUR") => Ordering::Greater,
            ("USD", "GBP") | ("GBP", "USD") => Ordering::Equal,
            ("USD", _) => Ordering::Less,
            (_, "USD") => Ordering::Greater,
            ("GBP", _) => Ordering::Less,
            (_, "GBP") => Ordering::Greater,
            _ => Ordering::Equal,
        }
    });
    let rendered = CurrenciesTemplate {
        date: &date.value,
        currencies: date.currencies.as_slice(),
    }
    .render()
    .map_err(|e| warp::reject::custom(e))?;

    Ok(warp::reply::html(rendered))
}