# Admin User Guide: 管理中心 (Admin Center)

**Feature**: `003-admin-center`
**Audience**: Administrators (Normal / System / Super) using the admin center web UI.

---

## 1. Roles at a Glance

| Role               | role code | Can view dashboard | Manage own profile | List admins | Create / edit / delete admins | Disable other admins | Delete / disable super admin |
| ------------------ | --------- | ------------------ | ------------------ | ----------- | ----------------------------- | -------------------- | ---------------------------- |
| Normal (普通)       | 1         | ✓                  | ✓                  | ✗           | ✗                             | ✗                    | ✗                            |
| System (系统)       | 2         | ✓                  | ✓                  | ✓           | ✓ (Normal only)               | ✓ (Normal only)      | ✗                            |
| Super (超级)        | 3         | ✓                  | ✓                  | ✓           | ✓ (any role)                  | ✓                    | ✗ (last super protected)      |

Rules enforced by the backend regardless of UI state:

- A Super admin cannot be **deleted**.
- The **last** active Super admin cannot be **disabled** (would lock the system out).
- Phone numbers must be unique across all admins.

---

## 2. Logging In

1. Navigate to the admin center URL (e.g. `https://admin.example.com`).
2. Enter the **phone number** (11 digits) and **password**.
3. Click **登录** (Login).

If the phone or password is wrong, an error message is shown. After **5 consecutive failed attempts** the account is **temporarily locked** for 15 minutes — contact a System or Super admin to wait it out or reset the password.

On successful login you are redirected to the **Dashboard**. Your last-login timestamp is updated automatically.

### Forgot password

There is no self-service password reset. Ask a System or Super admin to edit your account and set a new password.

---

## 3. Dashboard

The dashboard shows:

- **Total admins** — count of all admin accounts.
- **Online admins** — admins who logged in within the last 24 hours.
- **Today's logins** — successful logins today (local time).
- **Recent login records** — most recent logins with phone, IP, success/failure, and timestamp.

Data refreshes when you navigate to the page; some deployments enable auto-refresh.

---

## 4. Admin Management (System / Super only)

Navigate to **管理员管理** (Admin Management) in the side menu. Normal admins do not see this menu.

### 4.1 Listing

- The table is **paginated** (default 10 rows per page).
- Columns: ID, phone, nickname, role, status (Active / Disabled), created at, last login.
- Use the page controls at the bottom to navigate.

### 4.2 Create an admin

1. Click **新增管理员** (Add Admin).
2. Fill in:
   - **Phone** — 11 digits, unique.
   - **Nickname** — up to 20 characters.
   - **Password** — minimum length enforced by the backend; use a strong value.
   - **Role** — System admins can only create **Normal** admins; Super admins can create any role.
3. Submit. Newly created admins are **Active** by default.

### 4.3 Edit an admin

1. Click **编辑** (Edit) on the relevant row.
2. You may change nickname, phone, role (Super only), or set a new password (leave blank to keep current).
3. Submit to save.

### 4.4 Disable / enable an admin

1. Click the status toggle on the row.
2. A confirmation dialog appears.
3. Confirm to apply.

Disabled admins cannot log in. The system **refuses to disable the last active Super admin**.

### 4.5 Delete an admin

1. Click **删除** (Delete) on the row.
2. Confirm in the dialog.

The system **refuses to delete a Super admin** outright.

---

## 5. Logging Out

Click your nickname in the top-right and select **退出登录** (Logout). The JWT is discarded client-side and the session ends. You will be redirected to the login page.

---

## 6. Permission Boundaries (cheat sheet)

If you try to perform an action your role does not allow, you will see either:

- **The control is hidden** in the UI (e.g. delete button absent), or
- **A "权限不足" (insufficient permission) error** if you call the API directly.

The backend is the source of truth: hiding a button never replaces a server-side check.

---

## 7. Common Tasks

| Goal                                          | Steps                                                                                         |
| --------------------------------------------- | --------------------------------------------------------------------------------------------- |
| Promote a Normal admin to System              | Super admin → Admin Management → Edit row → change role to System → save.                     |
| Rotate the initial super-admin password       | Log in as that Super admin → Edit own row → set new password.                                  |
| Disable a departing employee's account        | System/Super → Admin Management → toggle status to Disabled on their row.                     |
| Investigate a suspicious login                | Dashboard → Recent login records → match phone, IP, and timestamp; failed attempts are shown. |
| Unlock a temporarily locked account immediately | Wait 15 minutes, or ask an operator to clear the Redis key `login:fail:<phone>`.             |

---

## 8. Error Codes

| Code  | Meaning                                |
| ----- | -------------------------------------- |
| 1001  | Login failed (wrong password)          |
| 1002  | Account disabled                       |
| 1003  | Account temporarily locked             |
| 1004  | Token invalid or expired               |
| 2001  | Insufficient permission                |
| 3001  | Admin not found                        |
| 3002  | Phone already in use                   |
| 3003  | Cannot delete a super admin            |
| 3004  | Cannot disable the last super admin    |

When raising a ticket, include the error code and approximate timestamp.

---

## 9. Security Tips

- **Never share** your account; each admin should have their own credentials so audit trails (login records) remain meaningful.
- Use a **password manager**; backend stores only bcrypt hashes — you cannot retrieve a forgotten password.
- Log out from shared devices.
- Report suspicious login records (unfamiliar IPs, off-hours logins) to a Super admin immediately.
