//! What removing a set of images can actually free: only layers that no image left behind keeps. Pure; the caller supplies
//! what Docker reported. Removing an image removes a reference. A layer is freed only when its last reference goes, so two
//! images with the same layers free nothing while one of them stays, and free their whole content when both go.

use std::collections::BTreeSet;

/// One image as Docker reports it: its own total size, shared layers included, and its layer diff ids, base first.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImageLayers {
    pub id: String,
    pub size: u64,
    pub layers: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Estimate {
    /// Bytes expected to be freed. When `exact` is false this is a lower bound, never more than what removal frees.
    pub bytes: u64,
    /// False when Docker's figures could not show the whole answer (missing layer data, a shared prefix that is no
    /// image on its own, or removed images whose exclusive layers overlap).
    pub exact: bool,
}

fn common_prefix(a: &[String], b: &[String]) -> usize {
    a.iter().zip(b).take_while(|(x, y)| x == y).count()
}

/// Layers are content-addressed and shared by prefix: an image's layer list is its base's list plus its own. So the part of an
/// image that stays is its longest prefix shared with an image that is kept, and its exclusive part is the rest. The size of
/// the kept prefix is known when a kept image is exactly that prefix (the base image a build started from), which is the usual
/// case; otherwise the image contributes nothing to the estimate rather than a guess.
pub fn estimate_reclaim(all: &[ImageLayers], remove: &BTreeSet<String>) -> Estimate {
    let kept: Vec<&ImageLayers> = all.iter().filter(|i| !remove.contains(&i.id)).collect();
    let mut exact = true;
    // images with identical layers are one group: their content is freed once, when the last of them goes
    let mut groups: Vec<(&Vec<String>, u64, usize)> = Vec::new(); // layers, size, shared prefix length with a kept image
    for img in all.iter().filter(|i| remove.contains(&i.id)) {
        if img.layers.is_empty() || img.size == 0 {
            exact = false;
            continue;
        }
        if groups.iter().any(|(l, _, _)| **l == img.layers) {
            continue;
        }
        let k = kept
            .iter()
            .map(|o| common_prefix(&img.layers, &o.layers))
            .max()
            .unwrap_or(0);
        groups.push((&img.layers, img.size, k));
    }
    let mut parts: Vec<u64> = Vec::new();
    for (layers, size, k) in &groups {
        if *k == layers.len() {
            parts.push(0); // every layer stays with a kept image
        } else if *k == 0 {
            parts.push(*size);
        } else if let Some(base) = kept.iter().find(|o| o.layers[..] == layers[..*k]) {
            parts.push(size.saturating_sub(base.size));
        } else {
            exact = false;
            parts.push(0);
        }
    }
    // two removed groups that share layers beyond what stays would be counted twice
    let overlapping = groups.iter().enumerate().any(|(i, a)| {
        groups[i + 1..].iter().any(|b| {
            let cp = common_prefix(a.0, b.0);
            cp > a.2 && cp > b.2
        })
    });
    if overlapping {
        return Estimate {
            bytes: parts.iter().copied().max().unwrap_or(0),
            exact: false,
        };
    }
    Estimate {
        bytes: parts.iter().sum(),
        exact,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn img(id: &str, size: u64, layers: &[&str]) -> ImageLayers {
        ImageLayers {
            id: id.into(),
            size,
            layers: layers.iter().map(|s| s.to_string()).collect(),
        }
    }
    fn set(ids: &[&str]) -> BTreeSet<String> {
        ids.iter().map(|s| s.to_string()).collect()
    }
    const BASE: [&str; 2] = ["b1", "b2"];

    // the real guest: a Node base image, an application release, and two rebuilds with the same layers
    fn world() -> Vec<ImageLayers> {
        vec![
            img("base", 250, &BASE),
            img("serving", 1950, &["b1", "b2", "s1", "s2"]),
            img("rollback", 1940, &["b1", "b2", "r1", "r2"]),
            img("leftover", 1940, &["b1", "b2", "r1", "r2"]),
        ]
    }

    #[test]
    fn removing_one_of_two_images_with_the_same_layers_frees_nothing() {
        let e = estimate_reclaim(&world(), &set(&["leftover"]));
        assert_eq!(
            e,
            Estimate {
                bytes: 0,
                exact: true
            }
        );
    }

    #[test]
    fn removing_both_frees_their_exclusive_layers_but_not_the_shared_base() {
        let e = estimate_reclaim(&world(), &set(&["leftover", "rollback"]));
        assert_eq!(
            e,
            Estimate {
                bytes: 1940 - 250,
                exact: true
            }
        );
    }

    #[test]
    fn an_image_nothing_else_shares_frees_its_whole_size() {
        let mut w = world();
        w.push(img("alone", 500, &["x1", "x2"]));
        assert_eq!(
            estimate_reclaim(&w, &set(&["alone"])),
            Estimate {
                bytes: 500,
                exact: true
            }
        );
    }

    #[test]
    fn a_shared_prefix_that_is_no_image_is_a_lower_bound_not_a_guess() {
        let w = vec![
            img("kept", 900, &["p1", "p2", "k1"]),
            img("gone", 800, &["p1", "p2", "g1"]),
        ];
        assert_eq!(
            estimate_reclaim(&w, &set(&["gone"])),
            Estimate {
                bytes: 0,
                exact: false
            }
        );
    }

    #[test]
    fn missing_layer_data_is_unknown_not_zero_savings() {
        let w = vec![img("a", 100, &[]), img("b", 0, &["l"])];
        assert_eq!(
            estimate_reclaim(&w, &set(&["a", "b"])),
            Estimate {
                bytes: 0,
                exact: false
            }
        );
    }

    #[test]
    fn removed_images_whose_exclusive_layers_overlap_are_not_added_twice() {
        let w = vec![
            img("base", 250, &BASE),
            img("a", 600, &["b1", "b2", "m", "a"]),
            img("b", 620, &["b1", "b2", "m", "b"]),
        ];
        let e = estimate_reclaim(&w, &set(&["a", "b"]));
        assert!(!e.exact && e.bytes <= 620 - 250, "{e:?}");
    }

    #[test]
    fn nothing_removed_frees_nothing_and_is_exact() {
        assert_eq!(
            estimate_reclaim(&world(), &set(&[])),
            Estimate {
                bytes: 0,
                exact: true
            }
        );
    }
}
