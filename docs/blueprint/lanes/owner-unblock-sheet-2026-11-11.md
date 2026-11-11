# Owner unblock sheet (2026-11-11)

This sheet answers one question: what has to happen before the rows that are
not COMPLETE can be. It groups the 989 open rows by what stands outside the
repository. The grouping was read from each row's own gap text on 2026-11-11
and is unverified: the register row is the grade, and a session re-reads the
code before it acts. Counts overlap where a row names more than one thing.

## Summary

| What stands outside the repository | Rows | Of which code also remains |
|---|---|---|
| Nothing (code or an in-repository design decision closes it) | 285 | 273 |
| A deployed environment | 343 | 265 |
| A billed service or a paid licence | 155 | 80 |
| An outside party (vendor, hosted provider, measured record) | 107 | 53 |
| Another row has to land first | 41 | 15 |
| A figure only the owner may give | 31 | 18 |
| A live-order path, which this platform never builds | 27 | 0 |

## 1. Deployment

One approval moves the most rows: applying the dev environment, with one
execution node enabled and running passes. Nothing is deployed today
(execution_nodes_enabled is false in dev and the node map is empty elsewhere).
Applying is billed and needs the owner to say so by name and to dispatch
infra.yml; an agent may not.

| What the gap names | Rows naming it |
|---|---|
| The dev environment applied (Cloud Run services, terraform apply) | 94 |
| An execution node deployed and running passes (decision 19) | 78 |
| A game day or drill on the deployed system | 31 |
| The GitOps control plane (Argo CD, Kargo) running | 30 |
| Metrics scraped, then workload_metrics_exist set | 18 |

## 2. Billed services and licences

Each line is a switch or a purchase. The owner names which to enable and
accepts its cost; the code then has something to read.

| What the gap names | Rows naming it |
|---|---|
| A data vendor or a data licence | 93 |
| Memorystore, Spanner or Bigtable | 70 |
| Cloud Storage buckets (enable_cloud_storage) | 40 |
| qip-ledgerd deployed and QIP_LEDGERD_URL set | 29 |
| BigQuery or the warehouse | 24 |
| A hosted quantum provider and a measured QPU record | 20 |
| A funded peer for the mesh (R12-MESH-10112) | 8 |

## 3. Figures only the owner may give

A limit, threshold, rate or cost is a person's to give. No session invents
one.

| Row | What is needed (unverified reading) |
|---|---|
| R12-AGENCY-1182 | Orchestration past shadow refused (ADR 0114 D8); graded on inspection; closes only if owner rewords requirement. |
| R12-AMBIENT-3068 | Stored risk-fit snapshot with horizon and bands (risk-desk decision), per-strategy premise recording, premise checks. |
| R12-ASSET-4070 | Add strategy/region/horizon keys to ApprovedThesis, kernel grouping, tests, gross bound; class/region budgets need RISK record. |
| R12-CAPITAL-10097 | Forecast consumer with Calibration::Measured gate, world-to-forecast binding table, three direction-rule tests with mutations; sample count owned by RISK. |
| R12-CAPITAL-4078 | Protected patch mounting yield-shelf.json and QIP_YIELD_SHELF_PATH after RISK record argues shelf, share, ladder horizons; plus default-start test. |
| R12-CAPITAL-4091 | A person must declare hedge instrument and ratio, and alternate venue with taker cost, before arms run by default. |
| R12-COVERAGE-5105 | ADR and owner sign-off on how the five-day liquidity floor applies to private funds and real assets. |
| R12-DATA-3008 | Owner decides: set cadence in later environments or narrow blueprint_arch guard; acting on a target needs mounted candidate catalogue. |
| R12-DATA-6006 | Owner must state a price of information; then add manifest publication delay and cost reading to desk attribution. |
| R12-EVENT-5089 | Set breadth plan in protected infrastructure/environments; owner must choose which markets to install. |
| R12-EXEC-1168 | ContinuousQuoting refusal stands; reopens only by owner's own ADR (O15). |
| R12-EXEC-5075 | Committed environment states QIP_VENUE_QUOTE_LIMITS with ratio interval (owner limits), or narrow ratio half to monitor. |
| R12-EXPAND-6030 | Declined by ADR 0158 d8 and 0199; unmet until owner changes the requirement's instantiate text. |
| R12-EXPAND-6036 | Declined by ADR 0150 A2 and 0190; needs measured posed sizes over a real universe and owner call on a theses cap. |
| R12-GCP-8042 | Correct current/evidence citations now; later org-mode folder log sink with tftest, needs owner org_id, audit retention and cost; PAM entitlement names owner's. |
| R12-GOV-10004 | Patch variables.tf for allowed-locations variable; owner must decide value; folder-scope shielded VM constraint is owner root. |
| R12-LEDGER-5008 | Needs committed record from a person naming jurisdiction and lot relief method; then ADR amending 0153 decision 7. |
| R12-LEDGER-5019 | No TaxCharge producer by decision; tax method, rate and jurisdiction are facts only the owner can give. |
| R12-MESH-5055 | Feed funding.rs into EdgeCosts at desk refresh; borrow rate must be stated by operator. |
| R12-MESH-5056 | Committed tfvars must state arbitrage costs; figures and jurisdictional tax source come from owner. |
| R12-MODEL-6002 | Unified training-data export for non-hedge paths; hedge capture held by owner decision 7. |
| R12-MODEL-6004 | Decide what a hedged or unwound path's alternative is, then build proposal queue and twin arm; held by owner decision 7. |
| R12-REASON-1132 | Neural coupling declined by ADR 0150 decision 7; closes only if owner rewords requirement. |
| R12-REASON-1133 | Neural-coupling clause declined by ADR 0150 decision 7; row stays PARTIAL unless owner rewords requirement. |
| R12-REASON-1134 | Neural coupling out of scope by decision; closes only if owner rewords requirement. |
| R12-REASON-1136 | Neural coupling and SMT/MILP declined by decision; closes only if owner rewords requirement. |
| R12-REASON-1179 | Remaining clauses (SMT/MILP, neural-to-symbolic, planner) declined by ADR 0150; closes only if owner rewords requirement. |
| R12-RISK-4108 | Scenario weights wait on committed binding table, calibrated source and RISK's minimum resolved-forecast count. |
| R12-RISK-4116 | Prove joint world outside factor library bounds and for capped-long/single-position books; applying joint-tail reserve needs owner authorisation. |
| R12-RISK-5044 | Arm with_net_edge_margin_bp in a binary, count slippage and impact; margin value and strategy edges need owner figure. |
| R12-SEC-10002 | Restrict workforce lists to group: in variables.tf; owner decision, no-org mode has no groups. |

## 4. Rows that would need a live-order path

These cannot be completed as written: the platform is paper trading only. They
stay as they are unless the owner rewords the requirement.

R12-AGENCY-1006, R12-AGENCY-1183, R12-AGENCY-1184, R12-AGENCY-6078,
R12-AGENCY-6092, R12-AGENCY-6093, R12-AGENCY-6094, R12-AGENCY-6095,
R12-AGENCY-6096, R12-AGENCY-6097, R12-AGENCY-6098, R12-AGENCY-6108,
R12-AGENCY-7028, R12-AGENCY-9062, R12-CAPITAL-4076, R12-CAPITAL-4081,
R12-CAPITAL-4084, R12-CAPITAL-5078, R12-COVERAGE-7187, R12-EXEC-5045,
R12-EXEC-7004, R12-GOV-10149, R12-GOV-3030, R12-GOV-6119, R12-LEDGER-5027,
R12-OBS-9158, R12-REFLEX-10074

## Row lists

### A deployed environment (343)

R12-AGENCY-1025, R12-AMBIENT-10108, R12-AMBIENT-1040, R12-AMBIENT-3110,
R12-AMBIENT-7200, R12-AMBIENT-9033, R12-API-10017, R12-API-10020,
R12-API-8113, R12-API-9058, R12-ARCH-1001, R12-ARCH-1008, R12-ARCH-1070,
R12-ARCH-7135, R12-ARCH-9014, R12-ARCH-9071, R12-ASSET-4073, R12-CAPITAL-1099,
R12-CAPITAL-4086, R12-CICD-10088, R12-CICD-10117, R12-CICD-10118,
R12-CICD-10182, R12-CICD-10203, R12-CICD-7116, R12-CICD-7119, R12-CICD-8030,
R12-CICD-8032, R12-CICD-8117, R12-CICD-9066, R12-CICD-9076, R12-CICD-9085,
R12-CICD-9097, R12-CICD-9101, R12-CICD-9103, R12-CICD-9107, R12-CICD-9108,
R12-CICD-9112, R12-COMMERCE-1087, R12-COMMERCE-1088, R12-COMMERCE-1089,
R12-COMMERCE-1090, R12-COMMERCE-1091, R12-COMMERCE-1093, R12-COMMERCE-1094,
R12-COMMERCE-1171, R12-COMMERCE-5090, R12-COMMERCE-5091, R12-COMMERCE-5092,
R12-COMMERCE-5093, R12-COMMERCE-5094, R12-COMMERCE-5095, R12-COMMERCE-5096,
R12-COMMERCE-5097, R12-CONTRACT-6110, R12-CONTRACT-7062, R12-CONTRACT-7063,
R12-CONTRACT-7064, R12-CONTRACT-7066, R12-CONTRACT-7068, R12-CONTRACT-7089,
R12-CONTRACT-7092, R12-COVERAGE-5106, R12-COVERAGE-7149, R12-COVERAGE-7150,
R12-COVERAGE-7152, R12-COVERAGE-7159, R12-COVERAGE-7160, R12-COVERAGE-7184,
R12-DATA-10033, R12-DATA-10079, R12-DATA-10081, R12-DATA-10164, R12-DATA-1033,
R12-DATA-1037, R12-DATA-1039, R12-DATA-1041, R12-DATA-1101, R12-DATA-1102,
R12-DATA-1103, R12-DATA-1104, R12-DATA-1105, R12-DATA-1106, R12-DATA-2003,
R12-DATA-2012, R12-DATA-2015, R12-DATA-2018, R12-DATA-2019, R12-DATA-2022,
R12-DATA-2023, R12-DATA-2025, R12-DATA-2026, R12-DATA-2027, R12-DATA-2028,
R12-DATA-2029, R12-DATA-7013, R12-DATA-7021, R12-DATA-8017, R12-DATA-8018,
R12-DATA-8019, R12-DATA-8112, R12-DATA-8120, R12-DATA-8124, R12-DATA-9054,
R12-DATA-9187, R12-EVENT-1170, R12-EVENT-5086, R12-EVENT-5087, R12-EVENT-5088,
R12-EVID-10106, R12-EVID-10107, R12-EVID-1038, R12-EVID-1107, R12-EVID-1108,
R12-EVID-1109, R12-EVID-1110, R12-EVID-1111, R12-EVID-1112, R12-EVID-1154,
R12-EVID-2032, R12-EVID-2035, R12-EVID-2036, R12-EVID-2038, R12-EVID-3081,
R12-EVID-7010, R12-EXEC-1169, R12-EXEC-5081, R12-EXPAND-6011, R12-EXPAND-6026,
R12-EXPAND-6029, R12-EXPAND-6031, R12-EXPAND-6038, R12-EXPAND-6052,
R12-EXPAND-6055, R12-EXPAND-6070, R12-FABRIC-10145, R12-FABRIC-10156,
R12-FABRIC-10161, R12-FABRIC-1027, R12-FABRIC-7034, R12-FABRIC-7038,
R12-FABRIC-7040, R12-FABRIC-7046, R12-FABRIC-7057, R12-FABRIC-7058,
R12-FABRIC-7142, R12-FABRIC-7211, R12-FABRIC-8078, R12-FABRIC-8093,
R12-FABRIC-8094, R12-FABRIC-8097, R12-FABRIC-8101, R12-FABRIC-8109,
R12-FINOPS-10137, R12-FINOPS-10138, R12-FINOPS-8040, R12-FINOPS-9186,
R12-GCP-10012, R12-GCP-10060, R12-GCP-10165, R12-GCP-7001, R12-GCP-7002,
R12-GCP-7005, R12-GCP-8035, R12-GCP-8036, R12-GCP-8060, R12-GCP-8116,
R12-GCP-8119, R12-GCP-9086, R12-GCP-9120, R12-GOV-10005, R12-GOV-6116,
R12-LEDGER-10163, R12-LEDGER-10179, R12-LEDGER-5004, R12-LEDGER-5006,
R12-LEDGER-5013, R12-LEDGER-5018, R12-LEDGER-5022, R12-LEDGER-5025,
R12-LEDGER-5026, R12-LEDGER-8140, R12-LEDGER-8141, R12-MESH-10075,
R12-MESH-10110, R12-MESH-10166, R12-MESH-1071, R12-MESH-1072, R12-MESH-1190,
R12-MESH-5053, R12-MESH-5057, R12-MESH-5062, R12-MESH-5066, R12-MESH-5068,
R12-MESH-7141, R12-MESH-7143, R12-MESH-8003, R12-MODEL-10064, R12-MODEL-10084,
R12-MODEL-1082, R12-MODEL-1083, R12-MODEL-4005, R12-MODEL-4020,
R12-MODEL-4023, R12-MODEL-4024, R12-MODEL-6022, R12-MODEL-9009, R12-OBS-10059,
R12-OBS-3118, R12-OBS-7017, R12-OBS-7018, R12-OBS-7123, R12-OBS-7127,
R12-OBS-9141, R12-OBS-9143, R12-OBS-9148, R12-OBS-9152, R12-OBS-9154,
R12-OBS-9155, R12-OBS-9159, R12-OBS-9160, R12-OBS-9161, R12-OBS-9162,
R12-OBS-9163, R12-QUANT-10086, R12-QUANT-10169, R12-QUANT-9042,
R12-REASON-10099, R12-REFLEX-10024, R12-REFLEX-1007, R12-REFLEX-10085,
R12-REFLEX-10162, R12-REFLEX-1186, R12-REFLEX-1188, R12-REFLEX-1196,
R12-REFLEX-5030, R12-REFLEX-5031, R12-REFLEX-5040, R12-REFLEX-5041,
R12-REFLEX-5047, R12-REFLEX-6012, R12-REFLEX-7003, R12-REFLEX-7137,
R12-REFLEX-8002, R12-REFLEX-8063, R12-REFLEX-8064, R12-REFLEX-8065,
R12-REFLEX-8066, R12-REFLEX-8067, R12-REFLEX-8069, R12-REFLEX-9114,
R12-REFLEX-9117, R12-RES-10119, R12-RES-10125, R12-RES-10127, R12-RES-10131,
R12-RES-10148, R12-RES-10151, R12-RES-10152, R12-RES-10153, R12-RES-10155,
R12-RES-10158, R12-RES-10186, R12-RES-7095, R12-RES-7102, R12-RES-7103,
R12-RES-7104, R12-RES-7107, R12-RES-7113, R12-RES-7114, R12-RES-8026,
R12-RES-8038, R12-RES-8039, R12-RES-9087, R12-RES-9166, R12-RES-9167,
R12-RES-9170, R12-RES-9171, R12-RES-9173, R12-RES-9176, R12-RISK-1043,
R12-RISK-4101, R12-RISK-4112, R12-RISK-4114, R12-RISK-4115, R12-RISK-9049,
R12-SEC-10003, R12-SEC-10013, R12-SEC-10050, R12-SEC-10157, R12-SEC-10201,
R12-SEC-7014, R12-SEC-8100, R12-SEC-9060, R12-SEC-9068, R12-SEC-9077,
R12-SEC-9078, R12-SEC-9123, R12-SEC-9127, R12-SEC-9131, R12-TICK-1002,
R12-TICK-1009, R12-TICK-10146, R12-TICK-1022, R12-TICK-1049, R12-TICK-1050,
R12-TICK-1051, R12-TICK-1121, R12-TICK-1124, R12-TICK-1125, R12-TICK-1126,
R12-TICK-1127, R12-TICK-1176, R12-TICK-1177, R12-TICK-2042, R12-TICK-2047,
R12-TICK-2052, R12-TICK-2055, R12-TICK-2058, R12-TICK-2059, R12-TICK-2061,
R12-TICK-2063, R12-TICK-2064, R12-TICK-2066, R12-TICK-2067, R12-TICK-2069,
R12-TICK-2070, R12-TICK-2073, R12-TICK-2076, R12-TICK-2081, R12-TICK-2082,
R12-TICK-2083, R12-TICK-2084, R12-TICK-2085, R12-TICK-2086, R12-TICK-2092,
R12-TICK-3096, R12-TICK-3099, R12-TICK-3100, R12-TICK-7019, R12-TICK-7199,
R12-WORLD-10090, R12-WORLD-7023

### A billed service or a paid licence (155)

R12-AMBIENT-7202, R12-AMBIENT-9001, R12-ARCH-8029, R12-ARCH-9012,
R12-ARCH-9028, R12-CAPITAL-9047, R12-CICD-10045, R12-CICD-10046,
R12-CICD-10116, R12-CICD-10160, R12-CICD-8008, R12-COVERAGE-7158,
R12-COVERAGE-7192, R12-DATA-10035, R12-DATA-10036, R12-DATA-10037,
R12-DATA-10040, R12-DATA-10080, R12-DATA-10206, R12-DATA-8006, R12-DATA-8014,
R12-DATA-8015, R12-DATA-8016, R12-DATA-8122, R12-DATA-8123, R12-DATA-8125,
R12-DATA-8126, R12-DATA-8127, R12-DATA-8128, R12-DATA-8130, R12-DATA-9052,
R12-DATA-9053, R12-DATA-9184, R12-DATA-9185, R12-EXPAND-6027,
R12-FABRIC-10027, R12-FABRIC-10178, R12-FABRIC-7011, R12-FABRIC-7035,
R12-FABRIC-7037, R12-FABRIC-7042, R12-FABRIC-7054, R12-FABRIC-7136,
R12-FABRIC-8004, R12-FABRIC-8073, R12-FABRIC-8075, R12-FABRIC-8080,
R12-FABRIC-8081, R12-FABRIC-8083, R12-FABRIC-8085, R12-FABRIC-8086,
R12-FABRIC-8091, R12-FABRIC-8092, R12-FABRIC-8096, R12-FABRIC-8099,
R12-FABRIC-9118, R12-FINOPS-10028, R12-FINOPS-8041, R12-FINOPS-9188,
R12-GCP-10009, R12-GCP-8037, R12-GCP-8053, R12-GCP-8056, R12-GCP-8057,
R12-GCP-8058, R12-GCP-8104, R12-LEDGER-10041, R12-LEDGER-7009,
R12-LEDGER-8007, R12-LEDGER-8132, R12-LEDGER-8137, R12-MESH-1010,
R12-MESH-10112, R12-MESH-10113, R12-MESH-1018, R12-MESH-1073, R12-MESH-1074,
R12-MESH-1075, R12-MESH-1166, R12-MESH-1167, R12-MESH-5052, R12-MESH-5065,
R12-MESH-5069, R12-MODEL-10065, R12-MODEL-7007, R12-MODEL-9002,
R12-MODEL-9119, R12-OBS-10089, R12-OBS-7115, R12-OBS-8082, R12-OBS-9074,
R12-OBS-9140, R12-OBS-9146, R12-QUANT-10066, R12-QUANT-10067, R12-QUANT-10095,
R12-QUANT-1013, R12-QUANT-10173, R12-QUANT-10190, R12-QUANT-10208,
R12-QUANT-4033, R12-QUANT-4034, R12-QUANT-4035, R12-QUANT-4036,
R12-QUANT-4037, R12-QUANT-4038, R12-QUANT-4039, R12-QUANT-4040,
R12-QUANT-4041, R12-QUANT-4042, R12-QUANT-4050, R12-QUANT-4054,
R12-QUANT-4055, R12-QUANT-4056, R12-QUANT-4057, R12-QUANT-4058,
R12-QUANT-4059, R12-QUANT-4060, R12-QUANT-4061, R12-QUANT-4062,
R12-QUANT-9041, R12-QUANT-9043, R12-QUANT-9044, R12-REASON-9005,
R12-REFLEX-10205, R12-REFLEX-9180, R12-RES-7112, R12-RES-8103, R12-RES-9168,
R12-RES-9169, R12-RES-9174, R12-RES-9177, R12-SEC-10031, R12-SEC-10053,
R12-SEC-10054, R12-SEC-10056, R12-SEC-9064, R12-SEC-9109, R12-SEC-9128,
R12-SEC-9132, R12-SEC-9134, R12-SEC-9137, R12-TICK-10184, R12-TICK-2056,
R12-TICK-3102, R12-TICK-7020, R12-TICK-8110, R12-WORLD-10042, R12-WORLD-3045,
R12-WORLD-3050, R12-WORLD-3075, R12-WORLD-3082, R12-WORLD-3085,
R12-WORLD-8129, R12-WORLD-9025

### An outside party (107)

R12-API-8054, R12-API-9057, R12-API-9059, R12-ARCH-10159, R12-ARCH-1192,
R12-ASSET-3017, R12-ASSET-4072, R12-CICD-9095, R12-CONTRACT-6113,
R12-COVERAGE-7155, R12-COVERAGE-7167, R12-DATA-1153, R12-DATA-2001,
R12-DATA-2005, R12-DATA-2006, R12-DATA-2007, R12-DATA-2008, R12-DATA-2009,
R12-DATA-2010, R12-DATA-3105, R12-EVENT-5083, R12-EVENT-5084, R12-EVENT-5085,
R12-EVENT-7145, R12-EVID-2037, R12-EXEC-10147, R12-EXEC-8068, R12-EXPAND-6040,
R12-EXPAND-6069, R12-FABRIC-7039, R12-FABRIC-7048, R12-FABRIC-8077,
R12-FINOPS-10135, R12-GCP-10001, R12-GCP-10010, R12-GCP-10011, R12-GCP-10014,
R12-GCP-8034, R12-GCP-8043, R12-GCP-8044, R12-GCP-8045, R12-GCP-8046,
R12-GCP-8047, R12-GCP-8048, R12-GCP-8049, R12-GCP-8050, R12-GCP-8051,
R12-GCP-8052, R12-GCP-8059, R12-GOV-9067, R12-LEDGER-5009, R12-LEDGER-5020,
R12-LEDGER-5023, R12-LEDGER-5024, R12-MODEL-1085, R12-MODEL-1157,
R12-MODEL-4004, R12-MODEL-4007, R12-OBS-10057, R12-OBS-9165, R12-QUANT-10068,
R12-QUANT-1077, R12-QUANT-1078, R12-QUANT-1079, R12-QUANT-1080,
R12-QUANT-1081, R12-QUANT-1145, R12-QUANT-1158, R12-QUANT-4032,
R12-QUANT-4046, R12-QUANT-4063, R12-QUANT-7016, R12-QUANT-7144,
R12-QUANT-9007, R12-QUANT-9040, R12-REASON-1024, R12-REASON-3053,
R12-REFLEX-5036, R12-REFLEX-5038, R12-REFLEX-8070, R12-RISK-4096,
R12-RISK-4097, R12-RISK-4103, R12-RISK-4107, R12-SEC-10006, R12-SEC-10019,
R12-SEC-10022, R12-SEC-10055, R12-SEC-8062, R12-SEC-9129, R12-SEC-9130,
R12-TICK-10172, R12-TICK-1053, R12-TICK-1175, R12-TICK-2043, R12-TICK-2045,
R12-TICK-2046, R12-TICK-2050, R12-TICK-2065, R12-TICK-2068, R12-TICK-3097,
R12-TICK-3098, R12-TICK-7138, R12-WORLD-3037, R12-WORLD-3043, R12-WORLD-3046,
R12-WORLD-3047

### Another row first (41)

R12-ARCH-7134, R12-ASSET-4065, R12-CAPITAL-10122, R12-CAPITAL-10174,
R12-CAPITAL-1161, R12-CAPITAL-1163, R12-CAPITAL-4087, R12-CAPITAL-4088,
R12-CAPITAL-9051, R12-CICD-10168, R12-CICD-9113, R12-COVERAGE-5104,
R12-COVERAGE-7153, R12-COVERAGE-7156, R12-COVERAGE-7162, R12-COVERAGE-7163,
R12-COVERAGE-7175, R12-COVERAGE-7178, R12-COVERAGE-7191, R12-COVERAGE-7193,
R12-COVERAGE-7194, R12-EXPAND-1119, R12-EXPAND-1173, R12-EXPAND-6034,
R12-EXPAND-6054, R12-EXPAND-7147, R12-FABRIC-8027, R12-FABRIC-8076,
R12-GCP-8105, R12-GOV-6118, R12-LEDGER-1164, R12-LEDGER-5005, R12-LEDGER-5010,
R12-LEDGER-5021, R12-MESH-5064, R12-MODEL-10104, R12-MODEL-4010,
R12-MODEL-4011, R12-REASON-9032, R12-REFLEX-5034, R12-RES-7099

