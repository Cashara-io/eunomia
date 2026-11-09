// Captured from origin/main 6116a00ff86fed992a917863e9a3515f70373b84.
// Only changes: function name and an unreachable arm for this PR's new Median variant.
fn compute_main(side: &Side, filters: &[Cond], entities: &[&String], values: &Values, props: &Props) -> Computed {
    let unit = side.property.as_ref().and_then(|p| props.get(p)).and_then(|p| p.unit.clone());
    let mut used = Vec::new();
    let mut nums = Vec::new();
    let mut distinct = HashSet::new();
    let mut other_units: BTreeMap<(String, String), usize> = BTreeMap::new();
    let mut div_zero = 0;
    let mut too_large = 0;
    let mut skipped = HashSet::new();
    for e in entities {
        let held = values.get(*e);
        let get = |p: &str| held.and_then(|h| h.get(p));
        if !filters.iter().chain(&side.filters).all(|c| matches(c, get(&c.filter.property))) {
            continue;
        }
        if let Some(r) = &side.row {
            match r.value(&get, props) {
                Ok(v) => {
                    nums.push(v);
                    used.push((*e).clone());
                    continue;
                }
                Err(Skip::Missing) => {}
                Err(Skip::Unit(p, u)) => *other_units.entry((p, u)).or_default() += 1,
                Err(Skip::DivZero) => div_zero += 1,
                Err(Skip::TooLarge) => too_large += 1,
            }
            skipped.insert((*e).clone());
            continue;
        }
        let Some(p) = &side.property else {
            used.push((*e).clone());
            continue;
        };
        let Some(h) = get(p) else {
            if side.agg.numeric_only() {
                skipped.insert((*e).clone());
            }
            continue;
        };
        if side.agg.numeric_only() {
            if let (Some(u), Some(want)) = (&h.unit, &unit)
                && u != want
            {
                *other_units.entry((p.clone(), u.clone())).or_default() += 1;
                skipped.insert((*e).clone());
                continue;
            }
            let Some(n) = h.stored.num else {
                skipped.insert((*e).clone());
                continue;
            };
            nums.push(n);
        } else if side.agg == Agg::CountDistinct {
            let s = &h.stored;
            distinct.insert(format!("{:?}|{:?}|{:?}|{:?}", s.num, s.text.as_deref().map(str::to_lowercase), s.boolean, s.date));
        }
        used.push((*e).clone());
    }
    let value = match side.agg {
        Agg::Count => Some(used.len() as f64),
        Agg::CountDistinct => Some(distinct.len() as f64),
        Agg::Sum => Some(nums.iter().sum()),
        Agg::Avg if !nums.is_empty() => Some(nums.iter().sum::<f64>() / nums.len() as f64),
        Agg::Min => nums.iter().copied().reduce(f64::min),
        Agg::Max => nums.iter().copied().reduce(f64::max),
        Agg::Avg | Agg::Ratio => None,
        Agg::Median => panic!("median did not exist on main"),
    };
    Computed { value, used, other_units, div_zero, too_large, skipped }
}
