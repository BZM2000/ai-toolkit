use anyhow::{Context, Result, anyhow};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sqlx::PgPool;
use tokio::try_join;

const MODULE_SUMMARIZER: &str = "summarizer";
const MODULE_TRANSLATE_DOCX: &str = "translate_docx";
const MODULE_GRADER: &str = "grader";
const MODULE_REVIEWER: &str = "reviewer";
const LEGACY_GRADER_PROMPT_PREFIX: &str = "You evaluate manuscripts in the domains";
const PROTOTYPE_GRADER_PROMPT: &str = r#"You are tasked with grading manuscripts in the areas of urban soundscape, architectural acoustics, and healthy habitat. Six prestige levels of well-known journals are listed below for reference, but you do not need to consider manuscript fit to specific journals; these are to convey the relative prestige of each level. For each manuscript, provide your educated guess—expressed as a percentage—for the chance it would be sent out for external review at each of the six journal levels. In making your estimates, consider overall quality, scope breadth, methodological novelty, interest to readership, workload, quality of writing, methodological rigour, and whether the results fully support the claims. Some manuscripts you grade may already be published articles, but please evaluate them as if they are new, without regard to where they were actually published. Note each lower level should have a equal or higher chance than the previous level.
*Level 1 - High-impact broad journals*
Nature Sustainability; Nature Human Behaviour; The Innovation; Science Bulletin; National Science Review; One Earth; Nature Communications; Science Advances; Proceedings of the National Academy of Sciences
*Level 2 - Top large-field journals*
Sustainable Cities and Society; Environment International; npj Urban Sustainability; Computers, Environment and Urban Systems; Cities; Communications Earth & Environment; Landscape and Urban Planning; Building and Environment; Journal of Environmental Psychology
*Level 3 - Good small-field journals*
Building Simulation; Environment and Behaviour; People and Nature; Ecological Indicators; Journal of Environmental Management; Environmental Research; Urban Forestry and Urban Greening; Urban Climate; Applied Psychology: An International Review; Frontiers of Architectural Research; Journal of Forestry Research; Journal of Building Engineering; Developments in the Built Environment; Environmental Research Letters; Environmental Health; Health & Place; Humanities & Social Sciences Communications; Applied Acoustics;
*Level 4 - Mediocre field-specialised journals*
Sustainability Science; Indoor Air; Journal of Exposure Science and Environmental Epidemiology; Applied Psychology: Health and Well-Being; Building Research & Information; Environment and Planning B: Urban Analytics and City Science; Journal of Leisure Research; Journal of Outdoor Recreation and Tourism; Journal of the Acoustical Society of America; Indoor and Built Environment
*Level 5 - Low-level field-specialised journals*
Forests; Land; Buildings; Frontiers in Psychology; Behavioural Sciences; Journal of Asian Architecture and Building Engineering; Noise & Health; Acta Acustica
*Level 6 - Journals that explicitly say do not require novelty*
Scientific Reports; Plos ONE; Royal Society Open Science; BMC Psychology; Heliyon; Applied Sciences; Sage Open; Sustainability; PeerJ; Environmental Research Communications
## Output Format
Your output must be a JSON object with:
- Keys Level 1 through Level 6, each mapping to an integer percentage (0–100) indicating your estimate of the chance the manuscript is sent for external review at that level.
- A single key "justification" with a one-sentence rationale for your scoring.
Example output:
{
  "Level 1": 0,
  "Level 2": 10,
  "Level 3": 50,
  "Level 4": 80,
  "Level 5": 90,
  "Level 6": 100,
  "justification": "The methodology is solid, but the novelty and breadth do not meet the expectations for the highest-impact journals."
}"#;

#[derive(Clone, Debug, Default)]
pub struct ModuleSettings {
    summarizer: Option<SummarizerSettings>,
    translate_docx: Option<DocxTranslatorSettings>,
    grader: Option<GraderSettings>,
    reviewer: Option<ReviewerSettings>,
}

impl ModuleSettings {
    pub async fn ensure_defaults(pool: &PgPool) -> Result<()> {
        let summarizer_models = serde_json::to_value(default_summarizer_models())?;
        let summarizer_prompts = serde_json::to_value(default_summarizer_prompts())?;
        let docx_models = serde_json::to_value(default_docx_models())?;
        let docx_prompts = serde_json::to_value(default_docx_prompts())?;
        let grader_models = serde_json::to_value(default_grader_models())?;
        let grader_prompts = serde_json::to_value(default_grader_prompts())?;
        let reviewer_models = serde_json::to_value(default_reviewer_models())?;
        let reviewer_prompts = serde_json::to_value(default_reviewer_prompts())?;

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

        let insert_reviewer = sqlx::query(
            "INSERT INTO module_configs (module_name, models, prompts) VALUES ($1, $2, $3)
             ON CONFLICT (module_name) DO NOTHING",
        )
        .bind(MODULE_REVIEWER)
        .bind(&reviewer_models)
        .bind(&reviewer_prompts)
        .execute(pool);

        let legacy_like = format!("{LEGACY_GRADER_PROMPT_PREFIX}%");
        let update_grader_prompt = sqlx::query(
            "UPDATE module_configs SET prompts = $1, updated_at = NOW()
             WHERE module_name = $2 AND prompts->>'grading_instructions' LIKE $3",
        )
        .bind(&grader_prompts)
        .bind(MODULE_GRADER)
        .bind(&legacy_like)
        .execute(pool);

        try_join!(
            insert_summarizer,
            insert_docx,
            insert_grader,
            insert_reviewer,
            update_grader_prompt
        )?;

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
                MODULE_REVIEWER => {
                    settings.reviewer = Some(parse_reviewer_settings(row.models, row.prompts)?);
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

    pub fn reviewer(&self) -> Option<&ReviewerSettings> {
        self.reviewer.as_ref()
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

#[derive(Clone, Debug)]
pub struct ReviewerSettings {
    pub models: ReviewerModels,
    pub prompts: ReviewerPrompts,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ReviewerModels {
    pub round1_model_1: String,
    pub round1_model_2: String,
    pub round1_model_3: String,
    pub round1_model_4: String,
    pub round1_model_5: String,
    pub round1_model_6: String,
    pub round1_model_7: String,
    pub round1_model_8: String,
    pub round2_model: String,
    pub round3_model: String,
}

impl Default for ReviewerModels {
    fn default() -> Self {
        default_reviewer_models()
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ReviewerPrompts {
    pub initial_prompt: String,
    pub initial_prompt_zh: String,
    pub secondary_prompt: String,
    pub secondary_prompt_zh: String,
    pub final_prompt: String,
    pub final_prompt_zh: String,
}

impl Default for ReviewerPrompts {
    fn default() -> Self {
        default_reviewer_prompts()
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

fn parse_reviewer_settings(models: Value, prompts: Value) -> Result<ReviewerSettings> {
    let models: ReviewerModels = serde_json::from_value(models)
        .map_err(|err| anyhow!("failed to parse reviewer models: {err}"))?;
    let prompts: ReviewerPrompts = serde_json::from_value(prompts)
        .map_err(|err| anyhow!("failed to parse reviewer prompts: {err}"))?;
    Ok(ReviewerSettings { models, prompts })
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
        grading_instructions: PROTOTYPE_GRADER_PROMPT.to_string(),
        keyword_selection: "You analyze an academic manuscript to identify its primary and secondary research focuses. Choose from the following keywords only:\n{{KEYWORDS}}\n\nOutput valid JSON with a single \"main_keyword\" (string) and up to three distinct items in \"peripheral_keywords\" (array). Peripheral keywords must differ from the main keyword. If none apply beyond the main topic, return an empty array for peripherals.".to_string(),
    }
}

fn default_reviewer_models() -> ReviewerModels {
    ReviewerModels {
        round1_model_1: "openrouter/openai/gpt-4o".to_string(),
        round1_model_2: "openrouter/anthropic/claude-3.5-sonnet".to_string(),
        round1_model_3: "openrouter/google/gemini-pro-1.5".to_string(),
        round1_model_4: "openrouter/meta-llama/llama-3.1-70b-instruct".to_string(),
        round1_model_5: "openrouter/qwen/qwen-2.5-72b-instruct".to_string(),
        round1_model_6: "openrouter/mistralai/mistral-large-2".to_string(),
        round1_model_7: "openrouter/x-ai/grok-2".to_string(),
        round1_model_8: "openrouter/deepseek/deepseek-chat".to_string(),
        round2_model: "openrouter/openai/gpt-5.0".to_string(),
        round3_model: "openrouter/openai/gpt-5.0".to_string(),
    }
}

fn default_reviewer_prompts() -> ReviewerPrompts {
    ReviewerPrompts {
        initial_prompt: r#"You should act as a reviewer for manuscripts sent to you. You should provide at least eight major and five minor points of concern for each manuscript. When you first receive the manuscript, read it through once with a focus on the wider context of the research. Ask critical questions such as: What research question(s) do the authors address? Do they justify the question's importance? What methods do the authors use—are they current, or is there a better method available? Are there fundamental problems with their strategy? Would additional experiments greatly improve the manuscript, and are they necessary for publication? Would other data help confirm the results? Were the results analyzed and interpreted correctly? Does the evidence support the conclusion? Will the results advance the field, and does the advance match the journal's standards? Is the manuscript written for the correct audience? Is it appropriately targeted regarding international and cultural differences? Does the manuscript fit together logically, clearly describing what was done and why? If the language quality impedes understanding, request correction before proceeding further. After the first reading, summarize the manuscript with one medium-length sentence stating what the authors did and one medium-length sentence stating what they found. Then, provide one sentence outlining the major problem points to introduce the detailed points below; this brief summary is sufficient. You can then proceed with a direct evaluation of each section. Title, abstract and keywords: Assess if the title accurately summarizes the study. If not, suggest changes. Does the abstract present a clear, concise summary? Can it be understood by the broader scientific audience? Point out inclusion of irrelevant information or missing key facts. Are keywords appropriate and do they reflect the manuscript's content? Introduction: Judge whether the introduction provides sufficient, essential background and clearly defines the study's aims. Note if information is missing, extraneous, or if aims are ambiguous or inconsistent with the rest of the manuscript. Suggest additional references only if they are critical. Materials and Methods: Assess whether enough detail is present to allow replication. Identify potential biases, insufficient controls, or inadequate description of subjects and procedures. Insist on clarity and objectivity in outcome measures and statistical analysis. Do not hesitate to point out flaws in methodology or missing crucial information. Suggest further experiments only if truly necessary for publication. Results and Figures: Check that figures and tables are clear, contain measures of uncertainty, and are not redundant with the main text. Recommend removal or consolidation of figures and summarize non-essential results where appropriate. Indicate any suspicious image manipulation to the editor. Interpretation should be left to the Discussion section unless combined by the journal. Statistics: Ensure that tests are appropriate for the data and check justification for sample size and sources of bias. Point out failures in experimental replication, confounding, sampling, randomization and reporting of statistical significance (p-values and thresholds). Reviewing review articles: Judge the breadth, accuracy, and timeliness of literature cited and whether the discussion is balanced and structured logically. Discussion and Conclusion: Evaluate whether authors interpret the results in context and acknowledge the study's limitations. Note if the conclusions are unsupported, overstated, or lacking discussion of relevant alternative interpretations or previous literature. References: Note needed citations, outdated references, over-reliance on one group, or lack of balance in supporting versus contradicting literature. Writing a Report: Address both major and minor points in a straightforward, direct manner. Avoid unnecessary compliments and do not preface critiques with positive remarks unless they are significant. Keep feedback specific and actionable and cite page and line numbers where relevant. Use clear, concise, and unambiguous language, especially when indicating necessary changes. Ensure all feedback prioritizes clarity and directness rather than gentleness or excessive encouragement. As a guide, you should be aiming for journals roughly at the level of Building and Environment, Applied Acoustics, or Journal of Environmental Management when reviewing. Note, do not use markdown grammar or emojis in your report."#.to_string(),
        initial_prompt_zh: r#"你将扮演一位稿件审稿人，需要为收到的每一份稿件提供至少**八个主要问题**和**五个次要问题**，请以**中文核心期刊**的标准来要求审阅的稿件。。 ### **初次审阅** 当你收到稿件时，请先通读一遍，重点关注研究的宏观背景。你需要思考以下关键问题： * 作者旨在解决什么研究问题？他们是否充分论证了该问题的重要性？ * 作者使用了什么研究方法？这些方法是否先进，或有无更好的替代方法？ * 他们的研究策略是否存在根本性缺陷？ * 补充实验能否显著提升稿件质量？这些实验对于发表是否**必要**？ * 其他数据是否有助于验证研究结果？ * 结果的分析和解释是否正确？证据是否能支持结论？ * 研究结果能否推动该领域的发展？其学术贡献是否符合期刊的标准？ * 稿件的读者定位是否准确？是否恰当考虑了国际和文化背景的差异？ * 稿件的逻辑结构是否严密？是否清晰地描述了研究内容（做了什么）和研究动机（为什么做）？ * 如果语言质量严重影响理解，请在继续审阅前要求作者修改。 ### **撰写初步总结** 通读后，请用一个中等长度的句子总结**作者的研究内容**，再用一个中等长度的句子总结**他们的主要发现**。然后，用一句话**概括稿件存在的主要问题**，以引出下文的具体意见。这个简短的总结即可。 --- ### **分章节评估** 接下来，你可以直接评估稿件的每个部分。 * **标题、摘要和关键词**：评估标题是否准确概括了研究。如不准确，提出修改建议。摘要是否清晰、简洁？是否能让更广泛的科学读者理解？指出其中包含的无关信息或缺失的关键事实。关键词是否恰当并能反映稿件核心内容？ * **引言**：判断引言是否提供了充足且必要的背景信息，并清晰地阐述了研究目的。指出是否存在信息缺失、冗余，或者研究目的模糊、与稿件其他部分不一致的问题。**仅在绝对必要时**才建议补充关键参考文献。 * **材料与方法**：评估细节是否足以让他人重复实验。识别潜在的偏见、不充分的对照组，或对研究对象和实验流程描述不足之处。坚持要求结果测量指标和统计分析方法的清晰性与客观性。直接指出方法论上的缺陷或缺失的关键信息。**仅在对发表至关重要时**才建议补充实验。 * **结果与图表**：检查图表是否清晰、包含不确定性度量（如误差棒），并且不与正文内容重复。建议删除或合并图表，并在适当之处总结非核心结果。若发现任何可疑的图像处理痕迹，应向编辑指出。除非期刊格式要求，否则结果的解读应保留在讨论部分。 * **统计分析**：确保所用的统计检验方法适用于数据类型，并检查样本量选择的合理性以及偏见的来源。指出在实验可重复性、混杂变量控制、抽样、随机化以及统计显著性（如p值和显著性阈值）报告方面存在的问题。 * **评审综述文章**：判断所引文献的广度、准确性和时效性，以及文章的论述是否平衡、结构是否合乎逻辑。 * **讨论与结论**：评估作者是否结合相关背景恰当地解读了研究结果，并承认了研究的局限性。指出结论是否存在证据不足、夸大其词，或缺少对其他合理解释或已有文献的讨论等问题。 * **参考文献**：指出需要补充的引文、过时的参考文献、过度依赖某一研究团队的文献，或在支持性与矛盾性文献的引用之间缺乏平衡的问题。 --- ### **撰写审稿报告的原则** * **直截了当**：以坦率、直接的方式提出主要和次要问题。 * **避免客套**：避免不必要的恭维。除非稿件有非常突出的优点，否则不要在提出批评前先说赞扬的话。 * **具体且可操作**：保持反馈意见具体且具有可操作性，并在相关处引用页码和行号。 * **语言明确**：使用清晰、简洁、明确的语言，尤其是在指出**必须修改**之处时。 * **以清晰为重**：确保所有反馈都以**清晰和直接**为首要原则，而不是温和的语气或过度的鼓励。 * 请注意，请**不要**使用Markdown格式或表情符号。"#.to_string(),
        secondary_prompt: r#"You are the head reviewer. You will receive up to eight independent reviews of the same academic manuscript. You should synthesize them into a single coherent review report. Generally speaking, points raised by multiple reviewers should be kept unless you really disagree with it. Conversely, points raised by only ine or two reviewers should be treated with a pinch of salt. You should only produce a single report and nothing else. Format of your report should be summary, points of concern, final recommendation, and nothing else. Generally, if two reviewers recommend rejection, you should seriously consider recommendating rejection. Note, do not use markdown grammar or emojis in your report."#.to_string(),
        secondary_prompt_zh: r#"你将扮演**首席审稿人**的角色。你会收到最多八份针对同一份学术稿件的独立审稿意见，你的任务是将它们**整合成一份连贯的综合审稿报告**。 在整合意见时，请遵循以下原则： * 对于**多位审稿人都提出的问题**，除非你本人有充分理由强烈反对，否则应当保留。 * 对于**仅由一两位审稿人提出的问题**，应持谨慎态度。 * 通常情况下，如果有**两位或以上审稿人建议拒稿**，你应认真考虑给出拒稿的最终建议。 你的输出**只能是一份报告**，不应包含任何其他内容，你的报告不应该提到存在多个审稿人这一事实。报告的格式必须严格遵循以下结构，且仅包含这三个部分： 1. **稿件总结** 2. **问题 (至少10点)** 3. **最终建议** * 请注意，请**不要**使用Markdown格式或表情符号。"#.to_string(),
        final_prompt: r#"You are the final fact-checker of an academic review process. For the review report below, use the manuscript as the source of truth, verify factual claims and citations, correct any mistakes, flag unsupported assertions, and produce a final, publication-ready report. Do not give feedback to individual reviewer points. Only make necessary changes. If any point is factual, leave it unchanged. Include a short "Corrections made" section after the report. Note, do not use markdown grammar or emojis in your report."#.to_string(),
        final_prompt_zh: r#"你将扮演学术评审流程中的**最终事实核查员**。 对于下方提供的审稿报告，你需要以**原始稿件**作为唯一的事实依据，核查报告中的事实性陈述和引文信息。你的任务是纠正任何与原稿件内容不符的错误，标记出报告中缺少稿件证据支持的论断，并最终生成一份准确无误、可供发布的**终版报告**。 请遵循以下规则： * **不要**对审稿人提出的观点本身进行评论或反馈。你的工作仅限于事实层面的核对。 * **只进行必要的修改**。如果报告中的某项陈述经核查属实，请保持原样，不做改动。 * 在报告正文结束后，附上一个简短的"**修改说明**"(Corrections made)部分，列出你所做的所有修改。 * 请注意，请**不要**使用Markdown格式或表情符号。"#.to_string(),
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

pub async fn update_reviewer_models(pool: &PgPool, models: &ReviewerModels) -> Result<()> {
    update_models(pool, MODULE_REVIEWER, models).await
}

pub async fn update_reviewer_prompts(pool: &PgPool, prompts: &ReviewerPrompts) -> Result<()> {
    update_prompts(pool, MODULE_REVIEWER, prompts).await
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
