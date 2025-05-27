use std::sync::Arc;

use warp::{path::FullPath, reject, Filter, Rejection};

/// Creates a Warp filter that compares full request path to the given string.
/// Convenient when the path for an endpoint isn't defined in code, but comes from configuration.
pub fn match_full_path(
    path: impl Into<String>,
) -> impl Filter<Extract = (), Error = Rejection> + Clone {
    let path = Arc::new(path.into());

    warp::path::full()
        .and_then(move |p: FullPath| {
            let path = path.clone();
            async move {
                if p.as_str() == path.as_str() {
                    Ok(())
                } else {
                    Err(reject::not_found())
                }
            }
        })
        .untuple_one()
}
