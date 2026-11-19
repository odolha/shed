//! A bench's links as typed and edited: `name url`, added, replaced and renamed.

use crate::ProjectLink;

/// A link as typed: `name url`, or just a URL (named by its host).
/// `localhost:3000` becomes `http://…`, other bare hosts `https://…`.
pub fn parse(text: &str) -> Option<ProjectLink> {
    let text = text.trim();
    let (name, url) = match text.rsplit_once(char::is_whitespace) {
        Some((name, url)) => (name.trim().to_string(), url.to_string()),
        None => (String::new(), text.to_string()),
    };
    let url = if url.contains("://") {
        url
    } else if url.starts_with("localhost") || url.starts_with("127.") {
        format!("http://{url}")
    } else {
        format!("https://{url}")
    };
    let host = url.split("://").nth(1)?.split(['/', '?', '#']).next()?.to_string();
    if host.is_empty() {
        return None;
    }
    Some(ProjectLink { name: if name.is_empty() { host } else { name }, url })
}

/// How a link is typed back: `name url`.
pub fn unparse(link: &ProjectLink) -> String {
    format!("{} {}", link.name, link.url)
}

/// `links` with `link` in place of the one at `at` (or of the one with its
/// name), else added at the end.
pub fn with(links: &[ProjectLink], link: ProjectLink, at: Option<usize>) -> Vec<ProjectLink> {
    let mut out: Vec<ProjectLink> = links.to_vec();
    match at.filter(|i| *i < out.len()).or_else(|| out.iter().position(|l| l.name == link.name)) {
        Some(i) => {
            out[i] = link;
            // Renamed onto another's name: that one goes.
            let name = out[i].name.clone();
            let mut j = 0;
            out.retain(|l| {
                j += 1;
                j - 1 == i || l.name != name
            });
            out
        }
        None => {
            out.push(link);
            out
        }
    }
}

/// `links` with the one at `i` called `name` (the address stays).
pub fn renamed(links: &[ProjectLink], i: usize, name: &str) -> Option<Vec<ProjectLink>> {
    let name = name.trim();
    let link = links.get(i)?;
    (!name.is_empty()).then(|| with(links, ProjectLink { name: name.to_string(), url: link.url.clone() }, Some(i)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn links_are_typed_name_then_url() {
        assert_eq!(parse("app localhost:3000").unwrap(), ProjectLink { name: "app".into(), url: "http://localhost:3000".into() });
        assert_eq!(parse("grafana.example.com/d/x").unwrap().name, "grafana.example.com");
        assert_eq!(parse("CI board https://ci.example/").unwrap().name, "CI board");
        assert!(parse("").is_none());
        let link = |n: &str| ProjectLink { name: n.into(), url: format!("https://{n}/") };
        let links = vec![link("a"), link("b")];
        assert_eq!(with(&links, link("c"), None), vec![link("a"), link("b"), link("c")]);
        assert_eq!(with(&links, link("c"), Some(0)), vec![link("c"), link("b")]);
        assert_eq!(with(&links, link("b"), Some(0)), vec![link("b")], "renamed onto another");
        let r = renamed(&links, 1, " My App ").unwrap();
        assert_eq!((r[1].name.as_str(), r[1].url.as_str()), ("My App", "https://b/"));
        assert!(renamed(&links, 1, "  ").is_none());
    }
}
