use std::collections::BTreeSet;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};
use std::time::SystemTime;

use serde::{Deserialize, Serialize};

use crate::{OperationId, ProjectIdentity, SessionIdentity};

/// 两个项目之间基于稳定证据得到的身份关系。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProjectIdentityMatch {
    Same,
    Different,
    Conflict,
    Unknown,
}

/// 比较项目稳定身份，不使用标题或路径等展示信息进行推断。
pub fn compare_project_identity(
    source: &ProjectIdentity,
    target: &ProjectIdentity,
) -> ProjectIdentityMatch {
    if source.project_id == target.project_id {
        if !source.biz_project_id.is_empty()
            && !target.biz_project_id.is_empty()
            && source.biz_project_id != target.biz_project_id
        {
            return ProjectIdentityMatch::Conflict;
        }
        return ProjectIdentityMatch::Same;
    }

    if source.biz_project_id.is_empty() || target.biz_project_id.is_empty() {
        return ProjectIdentityMatch::Unknown;
    }

    if source.biz_project_id == target.biz_project_id {
        ProjectIdentityMatch::Same
    } else {
        ProjectIdentityMatch::Different
    }
}

/// 用户选择的同步范围。自定义范围按三类稳定 ID 的并集展开。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum SyncScope {
    AllHistory,
    Custom {
        account_ids: Vec<String>,
        project_ids: Vec<String>,
        session_ids: Vec<SessionIdentity>,
    },
}

/// 计划绑定的目标文件证据；执行前必须重新读取并逐项比较。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TargetFileEvidence {
    pub db_fingerprint: String,
    pub wal_fingerprint: Option<String>,
    pub shm_fingerprint: Option<String>,
}

/// 由组合根从当前 fixture 读取并固定的计划证据上下文。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SyncPlanContext {
    pub created_at: SystemTime,
    pub platform_id: String,
    pub data_location_id: String,
    pub current_user_id: String,
    pub account_evidence_fingerprint: String,
    pub target_file_evidence: TargetFileEvidence,
    pub schema_fingerprint: String,
    pub mapping_version: String,
    pub schema_compatible: bool,
}

/// Planner 使用的会话状态，不包含正文。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlanSessionInput {
    pub identity: SessionIdentity,
    pub version_available: bool,
}

/// Planner 使用的项目状态。显示归属只用于展开用户选择，真实写前断言使用活动归属。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlanProjectInput {
    pub identity: ProjectIdentity,
    pub display_owner: String,
    pub current_live_owner: String,
    pub sessions: Vec<PlanSessionInput>,
    pub archived_only: bool,
}

/// 构建计划时由受信任后端提供的完整输入。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BuildSyncPlanInput {
    pub created_at: SystemTime,
    pub platform_id: String,
    pub data_location_id: String,
    pub current_user_id: String,
    pub account_evidence_fingerprint: String,
    pub target_file_evidence: TargetFileEvidence,
    pub schema_fingerprint: String,
    pub mapping_version: String,
    pub schema_compatible: bool,
    pub scope: SyncScope,
    pub projects: Vec<PlanProjectInput>,
}

/// 同步动作。T05 只生成不可变计划，不执行任何写入。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum PlanAction {
    FollowProject {
        project_id: String,
        from_user_id: String,
        to_user_id: String,
    },
    AttachSessions {
        source_project_id: String,
        target_project_id: String,
        session_ids: Vec<SessionIdentity>,
    },
}

/// 操作 manifest 使用的唯一状态集合；状态意图必须先于对应副作用持久化。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OperationState {
    Planned,
    BackingUp,
    BackupVerified,
    TargetWriting,
    TargetCommittedUnverified,
    TargetVerifying,
    CatalogReconciling,
    VerificationInconclusive,
    FailurePreserving,
    FailureSnapshotVerified,
    RestoreStaging,
    RestoreStaged,
    RestoreReplacing,
    RestoredVerifying,
    Completed,
    CancelledBeforeWrite,
    FailedSafe,
    NotApplied,
    RestoredVerified,
    ManualRecoveryRequired,
}

impl OperationState {
    /// 终态不会被后续协调自动覆盖，避免重启时重放已完成或需人工处理的操作。
    pub fn is_terminal(self) -> bool {
        matches!(
            self,
            Self::Completed
                | Self::CancelledBeforeWrite
                | Self::FailedSafe
                | Self::NotApplied
                | Self::RestoredVerified
                | Self::ManualRecoveryRequired
        )
    }
}

/// 同步执行的结构化结果，不包含原始数据库错误、路径或密钥。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum SyncPlanExecutionOutcome {
    Completed { affected_rows: u64 },
    PlanExpired { backups_preserved: bool },
    CancelledBeforeWrite { backups_preserved: bool },
    FailedBeforeWrite { backups_preserved: bool },
    FailedAfterWrite { backups_preserved: bool },
    ManualRecoveryRequired { backups_preserved: bool },
    UnsupportedPlan,
}

/// 跨命令共享的取消信号；进入目标写入后，执行器只读取但不再中断保护步骤。
#[derive(Debug, Clone, Default)]
pub struct OperationCancellation {
    requested: Arc<AtomicBool>,
}

impl OperationCancellation {
    pub fn new() -> Self {
        Self::default()
    }

    /// 记录用户取消意图；调用方必须由当前操作阶段决定是否接受该意图。
    pub fn request(&self) {
        self.requested.store(true, Ordering::Release);
    }

    /// 读取取消意图，不会改变状态或触发副作用。
    pub fn is_requested(&self) -> bool {
        self.requested.load(Ordering::Acquire)
    }
}

/// 计划排除原因。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PlanExclusionReason {
    AlreadyCurrent,
    ProjectIdentityConflict,
    ProjectIdentityUnknown,
    ArchivedOnly,
    DeletedProject,
    SchemaIncompatible,
    SessionVersionUnavailable,
    PartialProjectRequiresTarget,
}

/// 被排除的项目或会话及其原因。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PlanExclusion {
    pub project_id: String,
    pub session_id: Option<SessionIdentity>,
    pub reason: PlanExclusionReason,
}

/// 执行器需要在写入前后验证的逻辑断言。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum PlanAssertion {
    ProjectOwner {
        project_id: String,
        expected_user_id: String,
    },
    SessionProject {
        session_id: SessionIdentity,
        expected_project_id: String,
    },
}

/// 不可变同步计划。字段私有且不支持反序列化，外部只能读取 Planner 生成的结果。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SyncPlan {
    operation_id: OperationId,
    created_at: SystemTime,
    platform_id: String,
    data_location_id: String,
    current_user_id: String,
    account_evidence_fingerprint: String,
    target_file_evidence: TargetFileEvidence,
    schema_fingerprint: String,
    mapping_version: String,
    scope_snapshot: SyncScope,
    actions: Vec<PlanAction>,
    exclusions: Vec<PlanExclusion>,
    expected_before: Vec<PlanAssertion>,
    expected_after: Vec<PlanAssertion>,
}

impl SyncPlan {
    /// 返回本次计划固定的操作 ID；不提供从外部字符串恢复 ID 的入口。
    pub fn operation_id(&self) -> &OperationId {
        &self.operation_id
    }

    /// 返回计划绑定的数据位置标识，执行器不得改用其它位置。
    pub fn data_location_id(&self) -> &str {
        &self.data_location_id
    }

    /// 返回计划生成时固定的目标账号，仅供后端生成安全预览。
    pub fn current_user_id(&self) -> &str {
        &self.current_user_id
    }

    /// 返回计划绑定的写入前文件证据，供执行器临近写入时再次比较。
    pub fn target_file_evidence(&self) -> &TargetFileEvidence {
        &self.target_file_evidence
    }

    pub fn actions(&self) -> &[PlanAction] {
        &self.actions
    }

    pub fn exclusions(&self) -> &[PlanExclusion] {
        &self.exclusions
    }

    pub fn scope_snapshot(&self) -> &SyncScope {
        &self.scope_snapshot
    }

    /// 返回计划绑定的 schema 指纹；只用于承接意图失效判断，不包含正文。
    pub fn schema_fingerprint(&self) -> &str {
        &self.schema_fingerprint
    }

    /// 返回计划绑定的适配映射版本。
    pub fn mapping_version(&self) -> &str {
        &self.mapping_version
    }

    /// 返回写后关系断言；执行器使用它验证混合批量计划的最终状态。
    pub fn expected_after(&self) -> &[PlanAssertion] {
        &self.expected_after
    }

    /// 返回写前关系断言；诊断和撤销计划生成器可复用同一组稳定关系。
    pub fn expected_before(&self) -> &[PlanAssertion] {
        &self.expected_before
    }

    /// 只接受与构建时完全一致的账号、目标文件、schema 与 mapping 证据。
    pub fn matches_context(&self, context: &SyncPlanContext) -> bool {
        self.platform_id == context.platform_id
            && self.data_location_id == context.data_location_id
            && self.current_user_id == context.current_user_id
            && self.account_evidence_fingerprint == context.account_evidence_fingerprint
            && self.target_file_evidence == context.target_file_evidence
            && self.schema_fingerprint == context.schema_fingerprint
            && self.mapping_version == context.mapping_version
            && context.schema_compatible
    }

    /// 提交后目标数据库指纹会因本次已确认写入而变化，只复核非目标文件证据。
    pub fn matches_post_commit_context(&self, context: &SyncPlanContext) -> bool {
        self.platform_id == context.platform_id
            && self.data_location_id == context.data_location_id
            && self.current_user_id == context.current_user_id
            && self.account_evidence_fingerprint == context.account_evidence_fingerprint
            && self.schema_fingerprint == context.schema_fingerprint
            && self.mapping_version == context.mapping_version
            && context.schema_compatible
    }
}

/// 根据稳定身份、明确范围和目标账号证据生成确定性计划。
pub fn build_sync_plan(mut input: BuildSyncPlanInput) -> SyncPlan {
    input
        .projects
        .sort_by(|a, b| a.identity.project_id.cmp(&b.identity.project_id));
    for project in &mut input.projects {
        project.sessions.sort_by(|a, b| {
            (
                &a.identity.product_history_namespace,
                &a.identity.original_session_id,
            )
                .cmp(&(
                    &b.identity.product_history_namespace,
                    &b.identity.original_session_id,
                ))
        });
        project.sessions.dedup_by(|a, b| a.identity == b.identity);
    }

    let mut actions = Vec::new();
    let mut exclusions = Vec::new();
    let mut expected_before = Vec::new();
    let mut expected_after = Vec::new();

    if !input.schema_compatible {
        for project in selected_source_projects(&input) {
            exclusions.push(exclusion(
                &project.identity.project_id,
                None,
                PlanExclusionReason::SchemaIncompatible,
            ));
        }
        return finish_plan(input, actions, exclusions, expected_before, expected_after);
    }

    let target_projects: Vec<&PlanProjectInput> = input
        .projects
        .iter()
        .filter(|project| {
            !project.identity.soft_deleted && project.current_live_owner == input.current_user_id
        })
        .collect();

    for source in selected_source_projects(&input) {
        let project_id = &source.identity.project_id;
        if source.identity.soft_deleted {
            exclusions.push(exclusion(
                project_id,
                None,
                PlanExclusionReason::DeletedProject,
            ));
            continue;
        }
        if source.current_live_owner == input.current_user_id {
            exclusions.push(exclusion(
                project_id,
                None,
                PlanExclusionReason::AlreadyCurrent,
            ));
            continue;
        }
        if source.archived_only || source.sessions.is_empty() {
            exclusions.push(exclusion(
                project_id,
                None,
                PlanExclusionReason::ArchivedOnly,
            ));
            continue;
        }

        let selected = selected_sessions(source, &input.scope);
        let selected_ids: BTreeSet<SessionIdentity> = selected
            .iter()
            .filter(|session| session.version_available)
            .map(|session| session.identity.clone())
            .collect();
        for session in selected.iter().filter(|session| !session.version_available) {
            exclusions.push(exclusion(
                project_id,
                Some(session.identity.clone()),
                PlanExclusionReason::SessionVersionUnavailable,
            ));
        }
        if selected_ids.is_empty() {
            continue;
        }

        let matches: Vec<(&PlanProjectInput, ProjectIdentityMatch)> = target_projects
            .iter()
            .map(|target| {
                (
                    *target,
                    compare_project_identity(&source.identity, &target.identity),
                )
            })
            .collect();
        if matches
            .iter()
            .any(|(_, relation)| *relation == ProjectIdentityMatch::Conflict)
            || matches
                .iter()
                .filter(|(_, relation)| *relation == ProjectIdentityMatch::Same)
                .count()
                > 1
        {
            exclusions.push(exclusion(
                project_id,
                None,
                PlanExclusionReason::ProjectIdentityConflict,
            ));
            continue;
        }

        if let Some((target, _)) = matches
            .iter()
            .find(|(_, relation)| *relation == ProjectIdentityMatch::Same)
        {
            let target_session_ids: BTreeSet<SessionIdentity> = target
                .sessions
                .iter()
                .map(|session| session.identity.clone())
                .collect();
            let attach_ids: Vec<SessionIdentity> = selected_ids
                .iter()
                .filter(|session| !target_session_ids.contains(*session))
                .cloned()
                .collect();
            for existing in selected_ids
                .iter()
                .filter(|session| target_session_ids.contains(*session))
            {
                exclusions.push(exclusion(
                    project_id,
                    Some(existing.clone()),
                    PlanExclusionReason::AlreadyCurrent,
                ));
            }
            if !attach_ids.is_empty() {
                for session_id in &attach_ids {
                    expected_before.push(PlanAssertion::SessionProject {
                        session_id: session_id.clone(),
                        expected_project_id: project_id.clone(),
                    });
                    expected_after.push(PlanAssertion::SessionProject {
                        session_id: session_id.clone(),
                        expected_project_id: target.identity.project_id.clone(),
                    });
                }
                actions.push(PlanAction::AttachSessions {
                    source_project_id: project_id.clone(),
                    target_project_id: target.identity.project_id.clone(),
                    session_ids: attach_ids,
                });
            }
            continue;
        }

        if matches
            .iter()
            .any(|(_, relation)| *relation == ProjectIdentityMatch::Unknown)
        {
            exclusions.push(exclusion(
                project_id,
                None,
                PlanExclusionReason::ProjectIdentityUnknown,
            ));
            continue;
        }

        let all_available_ids: BTreeSet<SessionIdentity> = source
            .sessions
            .iter()
            .filter(|session| session.version_available)
            .map(|session| session.identity.clone())
            .collect();
        if selected_ids != all_available_ids {
            exclusions.push(exclusion(
                project_id,
                None,
                PlanExclusionReason::PartialProjectRequiresTarget,
            ));
            continue;
        }

        expected_before.push(PlanAssertion::ProjectOwner {
            project_id: project_id.clone(),
            expected_user_id: source.current_live_owner.clone(),
        });
        expected_after.push(PlanAssertion::ProjectOwner {
            project_id: project_id.clone(),
            expected_user_id: input.current_user_id.clone(),
        });
        actions.push(PlanAction::FollowProject {
            project_id: project_id.clone(),
            from_user_id: source.current_live_owner.clone(),
            to_user_id: input.current_user_id.clone(),
        });
    }

    finish_plan(input, actions, exclusions, expected_before, expected_after)
}

fn selected_source_projects(input: &BuildSyncPlanInput) -> Vec<&PlanProjectInput> {
    input
        .projects
        .iter()
        .filter(|project| {
            if matches!(input.scope, SyncScope::AllHistory) {
                return true;
            }
            match &input.scope {
                SyncScope::Custom {
                    account_ids,
                    project_ids,
                    session_ids,
                } => {
                    account_ids.contains(&project.display_owner)
                        || project_ids.contains(&project.identity.project_id)
                        || project
                            .sessions
                            .iter()
                            .any(|session| session_ids.contains(&session.identity))
                }
                SyncScope::AllHistory => true,
            }
        })
        .collect()
}

fn selected_sessions<'a>(
    project: &'a PlanProjectInput,
    scope: &SyncScope,
) -> Vec<&'a PlanSessionInput> {
    match scope {
        SyncScope::AllHistory => project.sessions.iter().collect(),
        SyncScope::Custom {
            account_ids,
            project_ids,
            session_ids,
        } => {
            if account_ids.contains(&project.display_owner)
                || project_ids.contains(&project.identity.project_id)
            {
                project.sessions.iter().collect()
            } else {
                project
                    .sessions
                    .iter()
                    .filter(|session| session_ids.contains(&session.identity))
                    .collect()
            }
        }
    }
}

fn exclusion(
    project_id: &str,
    session_id: Option<SessionIdentity>,
    reason: PlanExclusionReason,
) -> PlanExclusion {
    PlanExclusion {
        project_id: project_id.to_string(),
        session_id,
        reason,
    }
}

fn finish_plan(
    input: BuildSyncPlanInput,
    actions: Vec<PlanAction>,
    exclusions: Vec<PlanExclusion>,
    expected_before: Vec<PlanAssertion>,
    expected_after: Vec<PlanAssertion>,
) -> SyncPlan {
    SyncPlan {
        operation_id: OperationId::new(),
        created_at: input.created_at,
        platform_id: input.platform_id,
        data_location_id: input.data_location_id,
        current_user_id: input.current_user_id,
        account_evidence_fingerprint: input.account_evidence_fingerprint,
        target_file_evidence: input.target_file_evidence,
        schema_fingerprint: input.schema_fingerprint,
        mapping_version: input.mapping_version,
        scope_snapshot: input.scope,
        actions,
        exclusions,
        expected_before,
        expected_after,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn project(project_id: &str, biz_project_id: &str, display_name: &str) -> ProjectIdentity {
        ProjectIdentity {
            project_id: project_id.to_string(),
            biz_project_id: biz_project_id.to_string(),
            display_name: display_name.to_string(),
            soft_deleted: false,
        }
    }

    fn session(id: &str) -> PlanSessionInput {
        PlanSessionInput {
            identity: SessionIdentity::new("work_cn", id),
            version_available: true,
        }
    }

    fn plan_project(
        project_id: &str,
        biz_project_id: &str,
        owner: &str,
        sessions: &[&str],
    ) -> PlanProjectInput {
        PlanProjectInput {
            identity: project(project_id, biz_project_id, "同一标题"),
            display_owner: owner.to_string(),
            current_live_owner: owner.to_string(),
            sessions: sessions.iter().map(|id| session(id)).collect(),
            archived_only: false,
        }
    }

    fn input(scope: SyncScope, projects: Vec<PlanProjectInput>) -> BuildSyncPlanInput {
        BuildSyncPlanInput {
            created_at: SystemTime::UNIX_EPOCH,
            platform_id: "work_cn".to_string(),
            data_location_id: "fixture-location".to_string(),
            current_user_id: "target-user".to_string(),
            account_evidence_fingerprint: "account-fingerprint".to_string(),
            target_file_evidence: TargetFileEvidence {
                db_fingerprint: "db-fingerprint".to_string(),
                wal_fingerprint: None,
                shm_fingerprint: None,
            },
            schema_fingerprint: "schema-fingerprint".to_string(),
            mapping_version: "work_cn_v1".to_string(),
            schema_compatible: true,
            scope,
            projects,
        }
    }

    #[test]
    fn project_identity_matches_same_project_id() {
        let source = project("project-1", "biz-1", "来源标题");
        let target = project("project-1", "", "目标标题");
        assert_eq!(
            compare_project_identity(&source, &target),
            ProjectIdentityMatch::Same
        );
    }

    #[test]
    fn project_identity_matches_same_biz_project_id() {
        let source = project("source-project", "biz-1", "来源标题");
        let target = project("target-project", "biz-1", "目标标题");
        assert_eq!(
            compare_project_identity(&source, &target),
            ProjectIdentityMatch::Same
        );
    }

    #[test]
    fn project_identity_reports_conflicting_biz_id_for_same_project_id() {
        let source = project("project-1", "biz-source", "同一标题");
        let target = project("project-1", "biz-target", "同一标题");
        assert_eq!(
            compare_project_identity(&source, &target),
            ProjectIdentityMatch::Conflict
        );
    }

    #[test]
    fn project_identity_is_unknown_without_enough_stable_evidence() {
        let source = project("source-project", "", "同一标题");
        let target = project("target-project", "", "同一标题");
        assert_eq!(
            compare_project_identity(&source, &target),
            ProjectIdentityMatch::Unknown
        );
    }

    #[test]
    fn project_identity_ignores_same_display_name_when_stable_ids_differ() {
        let source = project("source-project", "biz-source", "同一标题");
        let target = project("target-project", "biz-target", "同一标题");
        assert_eq!(
            compare_project_identity(&source, &target),
            ProjectIdentityMatch::Different
        );
    }

    #[test]
    fn full_project_without_target_match_follows_project() {
        let plan = build_sync_plan(input(
            SyncScope::Custom {
                account_ids: vec![],
                project_ids: vec!["source-project".to_string()],
                session_ids: vec![],
            },
            vec![plan_project(
                "source-project",
                "biz-source",
                "source-user",
                &["session-1", "session-2"],
            )],
        ));

        assert_eq!(
            plan.actions(),
            &[PlanAction::FollowProject {
                project_id: "source-project".to_string(),
                from_user_id: "source-user".to_string(),
                to_user_id: "target-user".to_string(),
            }]
        );
    }

    #[test]
    fn immutable_plan_rejects_changed_account_or_file_evidence() {
        let plan = build_sync_plan(input(
            SyncScope::AllHistory,
            vec![plan_project(
                "source-project",
                "biz-source",
                "source-user",
                &["session-1"],
            )],
        ));
        let matching = SyncPlanContext {
            created_at: SystemTime::now(),
            platform_id: "work_cn".to_string(),
            data_location_id: "fixture-location".to_string(),
            current_user_id: "target-user".to_string(),
            account_evidence_fingerprint: "account-fingerprint".to_string(),
            target_file_evidence: TargetFileEvidence {
                db_fingerprint: "db-fingerprint".to_string(),
                wal_fingerprint: None,
                shm_fingerprint: None,
            },
            schema_fingerprint: "schema-fingerprint".to_string(),
            mapping_version: "work_cn_v1".to_string(),
            schema_compatible: true,
        };

        assert!(plan.matches_context(&matching));

        let mut changed_file = matching.clone();
        changed_file.target_file_evidence.db_fingerprint = "changed-db".to_string();
        assert!(
            !plan.matches_context(&changed_file),
            "目标 DB 指纹变化必须使不可变计划失效"
        );

        let mut changed_account = matching;
        changed_account.account_evidence_fingerprint = "changed-account".to_string();
        assert!(
            !plan.matches_context(&changed_account),
            "账号证据变化必须使不可变计划失效"
        );
    }

    #[test]
    fn selected_sessions_attach_to_reliably_matching_target_project() {
        let source = plan_project(
            "source-project",
            "biz-shared",
            "source-user",
            &["session-1", "session-2"],
        );
        let target = plan_project(
            "target-project",
            "biz-shared",
            "target-user",
            &["target-session"],
        );
        let plan = build_sync_plan(input(
            SyncScope::Custom {
                account_ids: vec![],
                project_ids: vec![],
                session_ids: vec![SessionIdentity::new("work_cn", "session-1")],
            },
            vec![source, target],
        ));

        assert_eq!(
            plan.actions(),
            &[PlanAction::AttachSessions {
                source_project_id: "source-project".to_string(),
                target_project_id: "target-project".to_string(),
                session_ids: vec![SessionIdentity::new("work_cn", "session-1")],
            }]
        );
    }

    #[test]
    fn partial_project_without_target_is_excluded_without_scope_expansion() {
        let plan = build_sync_plan(input(
            SyncScope::Custom {
                account_ids: vec![],
                project_ids: vec![],
                session_ids: vec![SessionIdentity::new("work_cn", "session-1")],
            },
            vec![plan_project(
                "source-project",
                "biz-source",
                "source-user",
                &["session-1", "session-2"],
            )],
        ));

        assert!(plan.actions().is_empty());
        assert_eq!(
            plan.exclusions(),
            &[PlanExclusion {
                project_id: "source-project".to_string(),
                session_id: None,
                reason: PlanExclusionReason::PartialProjectRequiresTarget,
            }]
        );
    }

    #[test]
    fn duplicate_session_already_in_target_is_not_attached_twice() {
        let source = plan_project(
            "source-project",
            "biz-shared",
            "source-user",
            &["same-session", "new-session"],
        );
        let target = plan_project(
            "target-project",
            "biz-shared",
            "target-user",
            &["same-session"],
        );
        let plan = build_sync_plan(input(
            SyncScope::Custom {
                account_ids: vec![],
                project_ids: vec!["source-project".to_string()],
                session_ids: vec![],
            },
            vec![source, target],
        ));

        assert_eq!(plan.actions().len(), 1);
        assert!(matches!(
            &plan.actions()[0],
            PlanAction::AttachSessions { session_ids, .. }
                if session_ids == &vec![SessionIdentity::new("work_cn", "new-session")]
        ));
        assert!(plan.exclusions().iter().any(|item| {
            item.reason == PlanExclusionReason::AlreadyCurrent
                && item.session_id == Some(SessionIdentity::new("work_cn", "same-session"))
        }));
    }

    #[test]
    fn identity_conflict_excludes_only_conflicting_project() {
        let source_conflict = plan_project(
            "conflict-project",
            "biz-source",
            "source-user",
            &["session-1"],
        );
        let target_conflict = plan_project(
            "conflict-project",
            "biz-target",
            "target-user",
            &["target-session"],
        );
        let source_ok = plan_project("ok-project", "biz-ok", "source-user", &["session-2"]);
        let plan = build_sync_plan(input(
            SyncScope::AllHistory,
            vec![source_conflict, target_conflict, source_ok],
        ));

        assert!(plan.actions().iter().any(|action| matches!(
            action,
            PlanAction::FollowProject { project_id, .. } if project_id == "ok-project"
        )));
        assert!(plan.exclusions().iter().any(|item| {
            item.project_id == "conflict-project"
                && item.reason == PlanExclusionReason::ProjectIdentityConflict
        }));
    }

    #[test]
    fn any_unknown_target_candidate_prevents_follow_project() {
        let source = plan_project(
            "source-project",
            "biz-source",
            "source-user",
            &["session-1"],
        );
        let unknown_target =
            plan_project("unknown-target", "", "target-user", &["target-session-1"]);
        let different_target = plan_project(
            "different-target",
            "biz-different",
            "target-user",
            &["target-session-2"],
        );
        let plan = build_sync_plan(input(
            SyncScope::AllHistory,
            vec![source, unknown_target, different_target],
        ));

        assert!(plan.actions().is_empty());
        assert!(plan.exclusions().iter().any(|item| {
            item.project_id == "source-project"
                && item.reason == PlanExclusionReason::ProjectIdentityUnknown
        }));
    }

    #[test]
    fn all_history_reports_projects_already_owned_by_target() {
        let plan = build_sync_plan(input(
            SyncScope::AllHistory,
            vec![plan_project(
                "target-project",
                "biz-target",
                "target-user",
                &["session-1"],
            )],
        ));

        assert!(plan.actions().is_empty());
        assert_eq!(
            plan.exclusions()[0].reason,
            PlanExclusionReason::AlreadyCurrent
        );
    }
}
