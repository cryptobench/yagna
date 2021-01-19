use diesel::backend::Backend;
use diesel::deserialize;
use diesel::serialize::Output;
use diesel::sql_types::Integer;
use diesel::types::{FromSql, ToSql};
use maplit::hashmap;
use serde::{Deserialize, Serialize};
use std::fmt::{Display, Formatter, Result as FmtResult};
use std::str::FromStr;
use ya_core_model::payment::local as pay_srv;

#[derive(AsExpression, FromSqlRow, PartialEq, Debug, Clone, Copy)]
#[sql_type = "Integer"]
pub enum Network {
    Mainnet = 1,
    Rinkeby = 4,
}

impl Default for Network {
    fn default() -> Self {
        Network::Rinkeby
    }
}

impl Into<pay_srv::Network> for Network {
    fn into(self) -> pay_srv::Network {
        match self {
            Network::Mainnet => pay_srv::Network {
                default_token: "GLM".to_string(),
                tokens: hashmap! {
                    "GLM".to_string() => "erc20-mainnet-glm".to_string()
                },
            },
            Network::Rinkeby => pay_srv::Network {
                default_token: "tGLM".to_string(),
                tokens: hashmap! {
                    "tGLM".to_string() => "erc20-rinkeby-tglm".to_string()
                },
            },
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, thiserror::Error)]
#[error("Invalid network: {0}")]
pub struct InvalidNetworkError(pub String);

impl FromStr for Network {
    type Err = InvalidNetworkError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "mainnet" => Ok(Network::Mainnet),
            "rinkeby" => Ok(Network::Rinkeby),
            _ => Err(InvalidNetworkError(s.to_string())),
        }
    }
}

impl Display for Network {
    fn fmt(&self, f: &mut Formatter<'_>) -> FmtResult {
        match *self {
            Network::Mainnet => f.write_str("mainnet"),
            Network::Rinkeby => f.write_str("rinkeby"),
        }
    }
}

impl<DB: Backend> ToSql<Integer, DB> for Network
where
    i32: ToSql<Integer, DB>,
{
    fn to_sql<W: std::io::Write>(&self, out: &mut Output<W, DB>) -> diesel::serialize::Result {
        (*self as i32).to_sql(out)
    }
}

impl<DB> FromSql<Integer, DB> for Network
where
    i32: FromSql<Integer, DB>,
    DB: Backend,
{
    fn from_sql(bytes: Option<&DB::RawValue>) -> deserialize::Result<Self> {
        Ok(match i32::from_sql(bytes)? {
            1 => Network::Mainnet,
            4 => Network::Rinkeby,
            _ => return Err(anyhow::anyhow!("invalid value").into()),
        })
    }
}
