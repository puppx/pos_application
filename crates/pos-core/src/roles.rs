use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    Admin,
    Staff,
}

impl Role {
    pub fn as_str(self) -> &'static str {
        match self {
            Role::Admin => "admin",
            Role::Staff => "staff",
        }
    }

    pub fn parse(s: &str) -> Option<Role> {
        match s {
            "admin" => Some(Role::Admin),
            "staff" => Some(Role::Staff),
            _ => None,
        }
    }

    pub fn can(self, p: Permission) -> bool {
        match self {
            Role::Admin => true,
            Role::Staff => matches!(p, Permission::Sell | Permission::ViewOwnSales),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Permission {
    Sell,
    ViewOwnSales,
    ViewAllSales,
    VoidSale,
    ManageProducts,
    AdjustStock,
    ManageUsers,
    ViewReports,
    ManageSettings,
    Backup,
}

/// An authenticated user. Created only by `Pos::login`, so holding one is
/// proof of a successful PIN check. The app layer keeps it on the Rust side;
/// the UI never sends a session back, it only gets a copy for display.
#[derive(Debug, Clone, Serialize)]
pub struct Session {
    pub user_id: i64,
    pub name: String,
    pub role: Role,
}

impl Session {
    pub fn require(&self, p: Permission) -> crate::Result<()> {
        if self.role.can(p) {
            Ok(())
        } else {
            Err(crate::PosError::Forbidden)
        }
    }
}
