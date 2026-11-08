#[derive(Clone)]
pub(super) struct Decimal {
    pub negative: bool,
    digits: String,
    scale: usize,
}

impl Decimal {
    pub fn new(value: f64) -> Self {
        // Display breaks shortest-decimal ties differently from JavaScript; Ryu uses its even tie rule.
        let mut buffer = ryu::Buffer::new();
        let text = buffer.format_finite(value);
        let text = text.trim_start_matches('-');
        let (mantissa, exponent) = text.split_once(['e', 'E']).unwrap_or((text, "0"));
        let fraction = mantissa.split_once('.').map_or(0, |(_, f)| f.len());
        let mut digits = mantissa.replace('.', "");
        let scale = fraction as i32 - exponent.parse::<i32>().unwrap_or(0);
        if scale < 0 {
            digits.push_str(&"0".repeat((-scale) as usize));
        }
        Self::normalise(value.is_sign_negative(), digits, scale.max(0) as usize)
    }

    fn normalise(negative: bool, digits: String, mut scale: usize) -> Self {
        let mut digits = digits.trim_start_matches('0').to_string();
        while scale > 0 && digits.ends_with('0') {
            digits.pop();
            scale -= 1;
        }
        if digits.is_empty() {
            return Self { negative: false, digits: "0".into(), scale: 0 };
        }
        Self { negative, digits, scale }
    }

    pub fn is_zero(&self) -> bool {
        self.digits == "0"
    }

    pub fn significant_places(&self) -> usize {
        if self.is_zero() { 0 } else { (self.scale + 2).saturating_sub(self.digits.len()) }
    }

    pub fn money_places(&self) -> usize {
        if self.scale > self.digits.len() + 1 { self.significant_places() } else { 2 }
    }

    fn render(negative: bool, digits: &str, scale: usize) -> String {
        let digits = format!("{:0>width$}", digits, width = scale + 1);
        let point = digits.len() - scale;
        let sign = if negative && digits.bytes().any(|b| b != b'0') { "-" } else { "" };
        if scale == 0 { format!("{sign}{digits}") } else { format!("{sign}{}.{}", &digits[..point], &digits[point..]) }
    }

    pub fn exact(&self) -> String {
        Self::render(self.negative, &self.digits, self.scale)
    }

    pub fn fixed(&self, places: usize) -> String {
        let digits = if places >= self.scale {
            format!("{}{}", self.digits, "0".repeat(places - self.scale))
        } else {
            let cut = self.scale - places;
            if cut > self.digits.len() {
                "0".to_string()
            } else {
                let keep = self.digits.len() - cut;
                let mut rounded = self.digits.as_bytes()[..keep].to_vec();
                if self.digits.as_bytes()[keep] >= b'5' {
                    let mut carry = true;
                    for digit in rounded.iter_mut().rev() {
                        if *digit < b'9' {
                            *digit += 1;
                            carry = false;
                            break;
                        }
                        *digit = b'0';
                    }
                    if carry { rounded.insert(0, b'1'); }
                }
                if rounded.is_empty() { "0".into() } else { String::from_utf8(rounded).unwrap() }
            }
        };
        Self::render(self.negative, &digits, places)
    }

    pub fn scaled(&self, places: usize) -> Option<i128> {
        self.fixed(places).replace('.', "").parse().ok()
    }

    pub fn difference(before: &Self, after: &Self) -> Self {
        let scale = before.scale.max(after.scale);
        let a = format!("{}{}", before.digits, "0".repeat(scale - before.scale)).trim_start_matches('0').to_string();
        let b = format!("{}{}", after.digits, "0".repeat(scale - after.scale)).trim_start_matches('0').to_string();
        let adding = before.negative != after.negative;
        let b_larger = (b.len(), &b) >= (a.len(), &a);
        let (large, small) = if adding || b_larger { (&b, &a) } else { (&a, &b) };
        let negative = if adding || b_larger { after.negative } else { !after.negative };
        let width = large.len().max(small.len());
        let large = format!("{large:0>width$}");
        let small = format!("{small:0>width$}");
        let mut carry = 0_i16;
        let mut digits = Vec::with_capacity(width + 1);
        // Decimal digit arithmetic also keeps an exact delta when scaled i128 values overflow.
        for (l, s) in large.bytes().rev().zip(small.bytes().rev()) {
            let n = (l - b'0') as i16 + if adding { (s - b'0') as i16 } else { -((s - b'0') as i16) } + carry;
            digits.push(b'0' + n.rem_euclid(10) as u8);
            carry = n.div_euclid(10);
        }
        if carry > 0 { digits.push(b'0' + carry as u8); }
        digits.reverse();
        Self::normalise(negative, String::from_utf8(digits).unwrap(), scale)
    }
}
