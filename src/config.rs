use anyhow::{Context, Result, anyhow};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sqlx::PgPool;
use tokio::try_join;

const MODULE_SUMMARIZER: &str = "summarizer";
const MODULE_TRANSLATE_DOCX: &str = "translate_docx";
const MODULE_GRADER: &str = "grader";

#[derive(Clone, Debug, Default)]
pub struct ModuleSettings {
    summarizer: Option<SummarizerSettings>,
    translate_docx: Option<DocxTranslatorSettings>,
    grader: Option<GraderSettings>,
}

impl ModuleSettings {
    pub async fn ensure_defaults(pool: &PgPool) -> Result<()> {
        let summarizer_models = serde_json::to_value(default_summarizer_models())?;
        let summarizer_prompts = serde_json::to_value(default_summarizer_prompts())?;
        let docx_models = serde_json::to_value(default_docx_models())?;
        let docx_prompts = serde_json::to_value(default_docx_prompts())?;
        let grader_models = serde_json::to_value(default_grader_models())?;
        let grader_prompts = serde_json::to_value(default_grader_prompts())?;

        let insert_summarizer = sqlx::query(
            "INSERT INTO module_configs (module_name, models, prompts) VALUES ($1, $2, $3)
             ON CONFLICT (module_name) DO NOTHING",
        )
        .bind(MODULE_SUMMARIZER)
        .bind(&summarizer_models)
        .bind(&summarizer_prompts)
        .execute(pool);

        let insert_docx = sqlx::query(
            "INSERT INTO module_configs (module_name, models, prompts) VALUES ($1, $2, $3)
             ON CONFLICT (module_name) DO NOTHING",
        )
        .bind(MODULE_TRANSLATE_DOCX)
        .bind(&docx_models)
        .bind(&docx_prompts)
        .execute(pool);

        let insert_grader = sqlx::query(
            "INSERT INTO module_configs (module_name, models, prompts) VALUES ($1, $2, $3)
             ON CONFLICT (module_name) DO NOTHING",
        )
        .bind(MODULE_GRADER)
        .bind(&grader_models)
        .bind(&grader_prompts)
        .execute(pool);

        try_join!(insert_summarizer, insert_docx, insert_grader)?;

        Ok(())
    }

    pub async fn load(pool: &PgPool) -> Result<Self> {
        let rows = sqlx::query_as::<_, ModuleConfigRow>(
            "SELECT module_name, models, prompts FROM module_configs",
        )
        .fetch_all(pool)
        .await
        .context("failed to load module configurations from database")?;

        let mut settings = ModuleSettings::default();
        for row in rows {
            match row.module_name.as_str() {
                MODULE_SUMMARIZER => {
                    settings.summarizer = Some(parse_summarizer_settings(row.models, row.prompts)?);
                }
                MODULE_TRANSLATE_DOCX => {
                    settings.translate_docx = Some(parse_docx_settings(row.models, row.prompts)?);
                }
                MODULE_GRADER => {
                    settings.grader = Some(parse_grader_settings(row.models, row.prompts)?);
                }
                other => {
                    return Err(anyhow!("unknown module configuration found: {}", other));
                }
            }
        }

        Ok(settings)
    }

    pub fn summarizer(&self) -> Option<&SummarizerSettings> {
        self.summarizer.as_ref()
    }

    pub fn translate_docx(&self) -> Option<&DocxTranslatorSettings> {
        self.translate_docx.as_ref()
    }

    pub fn grader(&self) -> Option<&GraderSettings> {
        self.grader.as_ref()
    }
}

#[derive(Clone, Debug)]
pub struct SummarizerSettings {
    pub models: SummarizerModels,
    pub prompts: SummarizerPrompts,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SummarizerModels {
    pub summary_model: String,
    pub translation_model: String,
}

impl Default for SummarizerModels {
    fn default() -> Self {
        default_summarizer_models()
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SummarizerPrompts {
    pub research_summary: String,
    pub general_summary: String,
    pub translation: String,
}

impl Default for SummarizerPrompts {
    fn default() -> Self {
        default_summarizer_prompts()
    }
}

#[derive(Clone, Debug)]
pub struct DocxTranslatorSettings {
    pub models: DocxTranslatorModels,
    pub prompts: DocxTranslatorPrompts,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DocxTranslatorModels {
    pub translation_model: String,
}

impl Default for DocxTranslatorModels {
    fn default() -> Self {
        default_docx_models()
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DocxTranslatorPrompts {
    #[serde(rename = "en_to_cn")]
    pub en_to_cn: String,
    #[serde(rename = "cn_to_en")]
    pub cn_to_en: String,
}

impl Default for DocxTranslatorPrompts {
    fn default() -> Self {
        default_docx_prompts()
    }
}

#[derive(Clone, Debug)]
pub struct GraderSettings {
    pub models: GraderModels,
    pub prompts: GraderPrompts,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct GraderModels {
    pub grading_model: String,
    pub keyword_model: String,
}

impl Default for GraderModels {
    fn default() -> Self {
        default_grader_models()
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct GraderPrompts {
    pub grading_instructions: String,
    pub keyword_selection: String,
}

impl Default for GraderPrompts {
    fn default() -> Self {
        default_grader_prompts()
    }
}

#[derive(sqlx::FromRow)]
struct ModuleConfigRow {
    module_name: String,
    models: Value,
    prompts: Value,
}

fn parse_summarizer_settings(models: Value, prompts: Value) -> Result<SummarizerSettings> {
    let models: SummarizerModels = serde_json::from_value(models)
        .map_err(|err| anyhow!("failed to parse summarizer models: {err}"))?;
    let prompts: SummarizerPrompts = serde_json::from_value(prompts)
        .map_err(|err| anyhow!("failed to parse summarizer prompts: {err}"))?;
    Ok(SummarizerSettings { models, prompts })
}

fn parse_docx_settings(models: Value, prompts: Value) -> Result<DocxTranslatorSettings> {
    let models: DocxTranslatorModels = serde_json::from_value(models)
        .map_err(|err| anyhow!("failed to parse DOCX translator models: {err}"))?;
    let prompts: DocxTranslatorPrompts = serde_json::from_value(prompts)
        .map_err(|err| anyhow!("failed to parse DOCX translator prompts: {err}"))?;
    Ok(DocxTranslatorSettings { models, prompts })
}

fn parse_grader_settings(models: Value, prompts: Value) -> Result<GraderSettings> {
    let models: GraderModels = serde_json::from_value(models)
        .map_err(|err| anyhow!("failed to parse grader models: {err}"))?;
    let prompts: GraderPrompts = serde_json::from_value(prompts)
        .map_err(|err| anyhow!("failed to parse grader prompts: {err}"))?;
    Ok(GraderSettings { models, prompts })
}

fn default_summarizer_models() -> SummarizerModels {
    SummarizerModels {
        summary_model: "openrouter/anthropic/claude-3-haiku".to_string(),
        translation_model: "openrouter/openai/gpt-4o-mini".to_string(),
    }
}

fn default_summarizer_prompts() -> SummarizerPrompts {
    SummarizerPrompts {
        research_summary: "You are an academic assistant. Write a detailed summary of the following research paper text. The summary should be approximately 800 words and cover these sections clearly:\n1. **Research Question/Objective:** State the main question or goal (~75 words).\n2. **Methodology:** Describe the methods, data collection, analysis techniques, tools, and participant/sample information (~400 words). Include specific details and quantitative information where available.\n3. **Findings/Results:** Present the key findings and results, including significant data points, statistical outcomes, or main observations (~400 words). Be specific and quantitative.\n4. **Discussion/Conclusion:** Briefly discuss the implications of the findings and the main conclusion (~75 words).\nStructure the output clearly. Do not use markdown formatting. Focus on factual reporting based only on the provided text.".to_string(),
        general_summary: "You are an assistant tasked with summarizing documents. Provide a concise yet comprehensive summary of the following text, aiming for approximately 600 words. Highlight the main points, key arguments, significant data or figures mentioned, and any conclusions drawn. Include specific quantitative details if they are present and relevant to the core message. Structure the summary logically. Do not use markdown formatting. Base the summary only on the provided text.".to_string(),
        translation: "You are an expert translator for academic manuscripts from English (EN) to Chinese (CN). Maintain academic tone and style. Use the following EN -> CN glossary entries for consistent terminology (each line is EN -> CN):\n{{GLOSSARY}}\nPreserve citations, references, and technical terms.".to_string(),
    }
}

fn default_docx_models() -> DocxTranslatorModels {
    DocxTranslatorModels {
        translation_model: "openrouter/openai/gpt-4o-mini".to_string(),
    }
}

fn default_docx_prompts() -> DocxTranslatorPrompts {
    DocxTranslatorPrompts {
        en_to_cn: "You are an expert translator for academic manuscripts from English (EN) to Chinese (CN). Maintain formal academic tone and style in CN.\nUse the glossary consistently—each entry is EN -> CN:\n{{GLOSSARY}}\nThe user's input contains multiple paragraphs separated by the exact marker {{PARAGRAPH_SEPARATOR}}. Return the translated paragraphs with the same marker preserved between them.\nIf a paragraph is only a URL or citation, return it unchanged.".to_string(),
        cn_to_en: "You are an expert translator for academic manuscripts from Chinese (CN) to English (EN). Maintain formal academic tone and style in EN (British academic English preferred).\nUse the glossary consistently—each entry is CN -> EN:\n{{GLOSSARY}}\nThe user's input contains multiple paragraphs separated by the exact marker {{PARAGRAPH_SEPARATOR}}. Return the translated paragraphs with the same marker preserved between them.\nIf a paragraph is only a URL or citation, return it unchanged.".to_string(),
    }
}

fn default_grader_models() -> GraderModels {
    GraderModels {
        grading_model: "openrouter/openai/gpt-5.0-mini".to_string(),
        keyword_model: "openrouter/openai/gpt-5.0-mini".to_string(),
    }
}

fn default_grader_prompts() -> GraderPrompts {
    GraderPrompts {
        grading_instructions: "You evaluate manuscripts in the domains of urban soundscape, architectural acoustics, healthy habitat, and related multidisciplinary topics. Estimate the chance (in integer percentages) that the manuscript would be sent for external review at each prestige level listed below. Ensure percentages do not decrease as the prestige level decreases (Level 6 should be the largest value). Your assessment should consider methodological strength, novelty, relevance to readership, clarity of writing, workload for reviewers, and whether conclusions are supported by results.\n\nLevels of reference (higher to lower prestige):\nLevel 1 – Nature Sustainability; Nature Human Behaviour; The Innovation; Science Bulletin; National Science Review; One Earth; Nature Communications; Science Advances; Proceedings of the National Academy of Sciences\nLevel 2 – Sustainable Cities and Society; Environment International; npj Urban Sustainability; Computers, Environment and Urban Systems; Cities; Communications Earth & Environment; Landscape and Urban Planning; Building and Environment; Journal of Environmental Psychology\nLevel 3 – Building Simulation; Environment and Behavior; People and Nature; Ecological Indicators; Journal of Environmental Management; Environmental Research; Urban Forestry & Urban Greening; Urban Climate; Applied Psychology: An International Review; Frontiers of Architectural Research; Journal of Forestry Research; Journal of Building Engineering; Developments in the Built Environment; Environmental Research Letters; Environmental Health; Health & Place; Humanities & Social Sciences Communications; Applied Acoustics\nLevel 4 – Sustainability Science; Indoor Air; Journal of Exposure Science and Environmental Epidemiology; Applied Psychology: Health and Well-Being; Building Research & Information; Environment and Planning B: Urban Analytics and City Science; Journal of Leisure Research; Journal of Outdoor Recreation and Tourism; Journal of the Acoustical Society of America; Indoor and Built Environment\nLevel 5 – Forests; Land; Buildings; Frontiers in Psychology; Behavioral Sciences; Journal of Asian Architecture and Building Engineering; Noise & Health; Acta Acustica\nLevel 6 – Scientific Reports; PLOS ONE; Royal Society Open Science; BMC Psychology; Heliyon; Applied Sciences; SAGE Open; Sustainability; PeerJ; Environmental Research Communications\n\nRespond with a strict JSON object:\n{\n  \"Level 1\": <int>,\n  \"Level 2\": <int>,\n  \"Level 3\": <int>,\n  \"Level 4\": <int>,\n  \"Level 5\": <int>,\n  \"Level 6\": <int>,\n  \"justification\": \"Single sentence explanation\"\n}\nDo not include extra keys or commentary. If any value would violate the non-decreasing rule, adjust scores to satisfy it while keeping the overall distribution realistic.".to_string(),
        keyword_selection: "You analyze an academic manuscript to identify its primary and secondary research focuses. Choose from the following keywords only:\n{{KEYWORDS}}\n\nOutput valid JSON with a single \"main_keyword\" (string) and up to three distinct items in \"peripheral_keywords\" (array). Peripheral keywords must differ from the main keyword. If none apply beyond the main topic, return an empty array for peripherals.".to_string(),
    }
}

pub async fn update_summarizer_models(pool: &PgPool, models: &SummarizerModels) -> Result<()> {
    update_models(pool, MODULE_SUMMARIZER, models).await
}

pub async fn update_summarizer_prompts(pool: &PgPool, prompts: &SummarizerPrompts) -> Result<()> {
    update_prompts(pool, MODULE_SUMMARIZER, prompts).await
}

pub async fn update_docx_models(pool: &PgPool, models: &DocxTranslatorModels) -> Result<()> {
    update_models(pool, MODULE_TRANSLATE_DOCX, models).await
}

pub async fn update_docx_prompts(pool: &PgPool, prompts: &DocxTranslatorPrompts) -> Result<()> {
    update_prompts(pool, MODULE_TRANSLATE_DOCX, prompts).await
}

pub async fn update_grader_models(pool: &PgPool, models: &GraderModels) -> Result<()> {
    update_models(pool, MODULE_GRADER, models).await
}

pub async fn update_grader_prompts(pool: &PgPool, prompts: &GraderPrompts) -> Result<()> {
    update_prompts(pool, MODULE_GRADER, prompts).await
}

async fn update_models<T: Serialize>(pool: &PgPool, module: &str, models: &T) -> Result<()> {
    let payload = serde_json::to_value(models)
        .map_err(|err| anyhow!("failed to serialize models payload: {err}"))?;
    let result = sqlx::query(
        "UPDATE module_configs SET models = $2, updated_at = NOW() WHERE module_name = $1",
    )
    .bind(module)
    .bind(payload)
    .execute(pool)
    .await?;

    if result.rows_affected() == 0 {
        return Err(anyhow!("module configuration not found for {module}"));
    }
    Ok(())
}

async fn update_prompts<T: Serialize>(pool: &PgPool, module: &str, prompts: &T) -> Result<()> {
    let payload = serde_json::to_value(prompts)
        .map_err(|err| anyhow!("failed to serialize prompts payload: {err}"))?;
    let result = sqlx::query(
        "UPDATE module_configs SET prompts = $2, updated_at = NOW() WHERE module_name = $1",
    )
    .bind(module)
    .bind(payload)
    .execute(pool)
    .await?;

    if result.rows_affected() == 0 {
        return Err(anyhow!("module configuration not found for {module}"));
    }
    Ok(())
}
