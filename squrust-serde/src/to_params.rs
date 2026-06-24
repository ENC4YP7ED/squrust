//! [`ToParams`]: convert Rust values into a positional bind-parameter list.

use squrust_sql::Value;

pub trait ToParams {
    fn to_params(self) -> Vec<Value>;
}

impl ToParams for () {
    fn to_params(self) -> Vec<Value> {
        vec![]
    }
}

impl ToParams for Vec<Value> {
    fn to_params(self) -> Vec<Value> {
        self
    }
}

impl<T: Into<Value> + Clone> ToParams for &[T] {
    fn to_params(self) -> Vec<Value> {
        self.iter().cloned().map(Into::into).collect()
    }
}

impl<T: Into<Value> + Clone, const N: usize> ToParams for [T; N] {
    fn to_params(self) -> Vec<Value> {
        self.into_iter().map(Into::into).collect()
    }
}

macro_rules! to_params_tuple {
    ($($name:ident),+) => {
        impl<$($name: Into<Value>),+> ToParams for ($($name,)+) {
            #[allow(non_snake_case)]
            fn to_params(self) -> Vec<Value> {
                let ($($name,)+) = self;
                vec![$($name.into()),+]
            }
        }
    };
}
to_params_tuple!(A);
to_params_tuple!(A, B);
to_params_tuple!(A, B, C);
to_params_tuple!(A, B, C, D);
to_params_tuple!(A, B, C, D, E);
to_params_tuple!(A, B, C, D, E, F);
to_params_tuple!(A, B, C, D, E, F, G);
to_params_tuple!(A, B, C, D, E, F, G, H);
to_params_tuple!(A, B, C, D, E, F, G, H, I);
to_params_tuple!(A, B, C, D, E, F, G, H, I, J);
to_params_tuple!(A, B, C, D, E, F, G, H, I, J, K);
to_params_tuple!(A, B, C, D, E, F, G, H, I, J, K, L);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unit_and_tuples() {
        assert!(().to_params().is_empty());
        let p = (1i64, "x", 2.5f64).to_params();
        assert_eq!(p.len(), 3);
        assert_eq!(p[0], Value::Integer(1));
        assert_eq!(p[1], Value::Text("x".into()));
        assert_eq!(p[2], Value::Real(2.5));
    }

    #[test]
    fn slices_and_arrays() {
        let v = vec![Value::Integer(1), Value::Null];
        assert_eq!(v.clone().to_params().len(), 2);
        let arr = [1i64, 2, 3];
        assert_eq!(arr.to_params().len(), 3);
    }
}
