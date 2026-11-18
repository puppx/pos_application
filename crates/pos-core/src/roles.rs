use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    Admin,
    /// Runs the floor and the menu: everything staff do, plus the menu, the
    /// floor plan, voids, all orders and the kitchen settings. Cannot delete
    /// anything, manage staff, back up or change the rest of the settings.
    Manager,
    Staff,
}

impl Role {
    pub fn as_str(self) -> &'static str {
        match self {
            Role::Admin => "admin",
            Role::Manager => "manager",
            Role::Staff => "staff",
        }
    }

    pub fn parse(s: &str) -> Option<Role> {
        match s {
            "admin" => Some(Role::Admin),
            "manager" => Some(Role::Manager),
            "staff" => Some(Role::Staff),
            _ => None,
        }
    }

    pub fn can(self, p: Permission) -> bool {
        match self {
            Role::Admin => true,
            Role::Manager => matches!(
                p,
                Permission::Sell
                    | Permission::ViewOwnOrders
                    | Permission::ViewAllOrders
                    | Permission::VoidOrder
                    | Permission::ManageMenu
                    | Permission::AdjustStock
                    | Permission::ManageFloorPlan
                    | Permission::ViewReports
                    | Permission::KitchenSettings
                    | Permission::AnyDiscount
            ),
            // Staff run the shift: they can see sales reports and close the day.
            Role::Staff => matches!(p, Permission::Sell | Permission::ViewOwnOrders | Permission::ViewReports),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Permission {
    /// Open orders, add items, send to kitchen, take payment.
    Sell,
    /// A discount over the staff limit (Settings, `discount_staff_max_pct`)
    /// without anyone else's PIN (admins and managers).
    AnyDiscount,
    ViewOwnOrders,
    ViewAllOrders,
    /// Remove items already sent to the kitchen, cancel such orders, void paid orders.
    VoidOrder,
    ManageMenu,
    AdjustStock,
    ManageFloorPlan,
    ManageUsers,
    /// Sales reports and End of day (closing the day, viewing and reprinting past days).
    ViewReports,
    ManageSettings,
    /// The kitchen settings only: drinks / desserts tickets, kitchen teams,
    /// quick notes (admins and managers).
    KitchenSettings,
    Backup,
    /// Deleting anything for good: orders, menu items, categories, options,
    /// floors, customers, backups (admins only).
    Delete,
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
