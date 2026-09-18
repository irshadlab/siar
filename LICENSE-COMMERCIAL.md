# SIAR Commercial License & Enterprise Exemption Agreement (SIAR-CEEL-1.0)

**Version 1.0 — Effective August 2026**

> **IMPORTANT NOTICE TO ENTERPRISE & COMMERCIAL USERS**:  
> The SIAR project employs a clear two-tier licensing model:
> 1. **Core Libraries & Crates (`crates/*`)** are dual-licensed under **MIT OR Apache-2.0**. They are 100% free and permissive for anyone (including commercial enterprises) to embed, link, and use in proprietary or open-source software without restriction.
> 2. **Standalone Applications & Daemons (`apps/*`)** are dual-licensed under the **GNU Affero General Public License Version 3 (AGPLv3)** for open-source use, and under this **Commercial License & Enterprise Exemption Agreement (SIAR-CEEL-1.0)** for commercial, proprietary, white-label, or closed-source SaaS/cloud-hosted deployments.  
> If you modify, rebrand, or host SIAR standalone applications or daemons within a proprietary product or network service without releasing your modifications under AGPLv3, **you are legally required to purchase and maintain an active SIAR Commercial Subscription**.

---

## 1. Preamble & Licensing Architecture

### 🕊️ Free for Humanity Commitment
**SIAR is, and will always remain, 100% free and open-source software for all of humanity.** 

- **Permissive Libraries (`crates/*`)**: All underlying Rust crates (`siar-crypto`, `siar-crypto-mls`, `siar-transport`, `siar-storage`, `siar-messaging`, `siar-routing-policy`, `siar-dtn-bundle`, `siar-connectivity`, etc.) are licensed under **MIT OR Apache-2.0** to maximize developer adoption, academic research, and interoperability across the ecosystem.
- **Copyleft Standalone Applications (`apps/*`)**: Complete end-user applications (`apps/android`, `apps/desktop`, `apps/cli`) and headless relay daemons (`apps/emergency-node`) are licensed under **GNU AGPLv3** to ensure that modifications to user-facing applications and network services remain open and accessible to the public.

### 💡 Fueling Development via Commercial Reinvestment
To prevent uncompensated enterprise exploitation of turnkey applications while guaranteeing perpetual freedom for humanity, SIAR provides a **Commercial Exemption Path (SIAR-CEEL-1.0)**:
- Commercial enterprises that modify or deploy SIAR standalone applications and daemons as proprietary SaaS platforms or white-labeled closed-source products can purchase a commercial subscription to lift AGPLv3 copyleft restrictions.
- **100% of commercial licensing revenues are directly reinvested to fuel the core engineering, security auditing, multi-platform maintenance, and ongoing development of SIAR for the benefit of humanity.**

---

## 2. Terms of Commercial Exemption Grant for Applications

Subject to your payment of applicable Commercial Subscription Fees and compliance with this Agreement, the SIAR Maintainers grant your entity a non-exclusive, worldwide, royalty-bearing (paid via subscription) commercial license to:

1. **Exemption from AGPLv3 Copyleft on Applications**:
   - You are fully exempt from Section 13 (Remote Network Interaction) of the GNU AGPLv3 for `apps/*`. You may host, run, scale, and provide cloud/SaaS messaging infrastructure built on SIAR applications without disclosing your application source code, proprietary algorithms, database schemas, or cloud orchestration software.
   - You may modify, rebrand, white-label, and adapt SIAR standalone applications (`apps/desktop`, `apps/android`, `apps/cli`, `apps/emergency-node`) for closed-source commercial distribution.

2. **Proprietary Modification & Distribution**:
   - You may modify, enhance, extend, and adapt SIAR application source code for internal enterprise use or external commercial distribution.
   - You are under no obligation to share or publish your proprietary modifications back to the public repository.

3. **Binary Redistribution**:
   - You may distribute compiled SIAR application binaries or embedded runtime components inside commercial mobile apps (iOS / Android), desktop software (Windows / macOS / Linux), embedded hardware devices (IoT / Automotive / Defense), or cloud container images under your own proprietary commercial End User License Agreement (EULA).

*(Note: Core crates under `crates/*` are already permissively licensed under MIT OR Apache-2.0 and do not require a commercial subscription for library linking.)*

---

## 3. Subscription & Recurring Revenue Terms

### 3.1 Tier Structure

The Commercial Exemption for applications is conditioned upon maintaining an active subscription based on organization size and deployment scale:

| Tier | Eligibility | Rights & Exemption Scope | Annual Subscription Fee |
| :--- | :--- | :--- | :--- |
| **Startup / Developer Tier** | Revenue < $2M USD & < 10 Employees | Closed-source app embedding/rebranding up to 50k active devices/nodes | **$2,400 / year** ($200/mo) |
| **Business / Growth Tier** | Revenue $2M - $20M USD | Closed-source app embedding/rebranding up to 500k active devices/nodes | **$12,000 / year** ($1,000/mo) |
| **Enterprise / Cloud Tier** | Revenue > $20M USD or Big-Tech / Telecommunications | Unlimited devices, SaaS network hosting, dedicated SLA & priority support | **Custom Quote** (Contact Licensing) |

### 3.2 Term, Renewal & Lapse

- **Billing Cycle**: Commercial subscriptions are billed annually or monthly in advance.
- **Continuous Validity**: The commercial exemption remains valid **only while your commercial subscription account is active and non-delinquent**.
- **Automatic Fallback on Lapse**: If your subscription lapses, is canceled, or fails payment beyond a 30-day grace period, the commercial exemption granted under SIAR-CEEL automatically terminates. Your deployment of `apps/*` immediately reverts to governance under the **GNU AGPLv3**, rendering un-disclosed proprietary SaaS or binary distribution an act of copyright infringement under applicable international copyright law.

---

## 4. Protection Boundary for Host Applications

Under an active Commercial Subscription:

1. **No Contamination of Host Infrastructure**: Your proprietary backend services, database schemas, authentication systems, frontend applications, and custom business logic that interface with SIAR software are completely protected from copyleft infection.
2. **Trademark Rights**: Commercial subscribers are granted a limited license to state *"Powered by SIAR Secure Mesh Infrastructure"* in corporate marketing materials and technical documentation.

---

## 5. Warranties, Service Level Agreements & Indemnification

1. **Warranty Tiering**: Unlike the AGPLv3 (which disclaims all warranties), active Enterprise Tier commercial subscribers receive an explicit limited warranty asserting that SIAR source code does not infringe upon third-party intellectual property or patent rights to the best knowledge of the maintainers.
2. **Dedicated Support SLA**: Enterprise tier subscribers receive access to private security advisory feeds, priority bug remediation, custom architecture reviews, and direct integration support.

---

## 6. Audit & Compliance

The SIAR maintainers reserve the right to request annual self-certification of commercial tier compliance. Audits will be conducted electronically and with minimal operational disruption to commercial subscribers.

---

## 7. Licensing Enquiries & Purchasing

To purchase a SIAR Commercial License, activate an Enterprise Exemption, or request a custom licensing quote:

- **Commercial Licensing Portal**: `https://siar.network/licensing`
- **Enterprise Contact Email**: `licensing@siar.network` / `irshad@siar.network`
- **GitHub Repository**: `https://github.com/irshadali5/siar`
