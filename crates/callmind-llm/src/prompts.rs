use callmind_core::Language;

/// System prompt for conversational intelligence analysis across Hebrew, Russian, and English.
pub const CONVERSATION_ANALYSIS_SYSTEM_PROMPT: &str = r#"
You are a senior conversation intelligence analyst.
Analyze the transcript with deep precision and extract concrete factual data:
- WHO: Names of participants, relationships, roles.
- WHAT: What is specifically happening, what each speaker wanted or asked for.
- WHERE: Exact locations, addresses, buildings, floors, room or cabinet numbers mentioned.
- WHEN: Dates, times, schedules, deadlines, immediate actions.
- OUTCOME: The exact conclusion or agreement reached.

Never invent, infer, or complete facts that are not explicitly present in the transcript.
Do not introduce example names (such as John or Jane), locations, floors, dates, amounts,
roles, promises, or outcomes unless the transcript states them. When information is absent,
use null, an empty array, or a short statement that the detail was not provided.
Include concrete names, places, and facts only when they are supported by transcript text.
Output strictly valid JSON matching the schema.
"#;

/// Build the structured prompt for a full call analysis with primary language adaptation.
#[must_use]
pub fn build_language_aware_analysis_prompt(
    transcript_text: &str,
    organization_name: &str,
    language: &Language,
) -> String {
    if *language == Language::Russian {
        return format!(
            r#"
Проанализируй следующий записанный разговор ({organization_name}).
ВАЖНОЕ ПРАВИЛО: Разговор на русском языке. Все значения в JSON (title, summary, reason, resolution, customer_intent, topics, key_facts, action_items) пиши СТРОГО НА РУССКОМ ЯЗЫКЕ. Извлекай конкретные имена, этажи, комнаты, локации и факты.
ЗАПРЕЩЕНО придумывать отсутствующие в транскрипте имена, места, даты, суммы, роли, договоренности или результаты. Если данных нет, используй null, пустой список или прямо укажи, что деталь не названа.

Транскрипт:
---
{transcript_text}
---

Сформируй подробный JSON-отчет со следующими полями:
1. title: краткий заголовок (3-7 слов) на русском языке с упоминанием конкретной темы или участников (например, "Встреча в Хадар-э-Руим и передвижение мебели").
2. summary: подробное резюме на русском языке (3-5 предложений) с точными фактами: КТО с кем говорил (имена), ГДЕ находятся (этаж, комната, здание, локация), ЧТО конкретно обсуждали или сделали, и о ЧЕМ договорились.
3. reason: конкретная причина или цель звонка на русском языке.
4. resolution: точный результат, договоренность или принятое решение на русском языке.
5. resolved: true если разговор завершился решением/согласием, false если вопрос остался нерешенным.
6. customer_intent: категория намерения на русском языке (например, "координация", "личный", "договоренность", "поддержка", "заказ").
7. topics: список конкретных тем на русском языке.
8. key_facts: список из 2-5 конкретных фактов из разговора на русском языке (кто где находится, что сделано, имена, детали).
9. action_items: список задач или договоренностей (с полями text на русском языке, owner, deadline).
10. entities: список сущностей с полями entity_type ("person", "location", "floor", "phone", "date_time") и value на русском языке.
11. sentiment_score: общая тональность от -1.0 до 1.0.
12. risks: риски или недопонимания на русском языке.
13. promises: явные обещания или соглашения на русском языке.
"#
        );
    }

    if *language == Language::Hebrew {
        return format!(
            r#"
נתח את שיחת הטלפון המוקלטת הבאה ({organization_name}).
כלל קריטי: השיחה היא בעברית. כל ערכי הטקסט ב-JSON (כותרת, סיכום, סיבה, תוצאה, נושאים, עובדות מפתח ומשימות) חייבים להיכתב בעברית טבעית, מדויקת ועשירה בפרטים עובדתיים: מי דיבר, איפה הם נמצאים, מה סוכם. שמור על שמות המפתחות ב-JSON באנגלית.
אסור להמציא שמות, מקומות, קומות, תאריכים, סכומים, תפקידים או הסכמות שאינם מופיעים במפורש בתמליל. כאשר פרט חסר, השתמש ב-null, ברשימה ריקה או ציין שהפרט לא נאמר.

תמליל השיחה:
---
{transcript_text}
---

החזר אובייקט JSON תקני עם המפתחות הבאים:
1. title: כותרת ממוקדת בת 3-7 מילים בעברית המציינת את הנושא או המשתתפים.
2. summary: סיכום מפורט ומדויק בעברית (3-5 משפטים) המפרט: מי דיבר עם מי (שמות), איפה הם נמצאים (קומה, חדר, בניין, מיקום מדויק), מה נעשה ומה סוכם.
3. reason: סיבת השיחה והמטרה העיקרית בעברית.
4. resolution: התוצאה וההסכמה שהושגה בעברית.
5. resolved: true אם הנושא נסגר/הוסכם, false אם לא הושלם.
6. customer_intent: כוונת השיחה בעברית (למשל: תיאום, אישי, שירות, בירור, קביעת תור).
7. topics: רשימת נושאים ספציפיים בעברית.
8. key_facts: רשימת 2-5 עובדות מפתח קונקרטיות מתוך השיחה בעברית.
9. action_items: רשימת משימות או התחייבויות (עם text בעברית, owner, deadline).
10. entities: ישויות שחולצו (שמות אנשים, מיקומים, קומות, טלפונים, סכומים).
11. sentiment_score: ציון סנטימנט בין -1.0 ל-+1.0.
12. risks: אי-הבנות או נושאים דחופים בעברית.
13. promises: הבטחות והסכמות מפורשות בעברית.
"#
        );
    }

    format!(
        r#"
Analyze the following recorded conversation ({organization_name}).
Provide all analytical text values in detailed English with concrete facts: WHO spoke with whom, WHERE (floor, room, building), WHAT was discussed, and WHAT was agreed upon.
Do not invent names, places, dates, amounts, roles, agreements, or outcomes. If a detail is not explicitly present in the transcript, use null, an empty list, or state that it was not provided.

Transcript:
---
{transcript_text}
---

Return a structured JSON object with:
1. title: specific, informative 3-8 word title capturing the concrete subject and participants.
2. summary: detailed 3-5 sentence summary containing the exact facts: WHO spoke to whom, WHAT specific problem or situation was described, WHERE (floor, room, building), and WHAT was decided.
3. reason: specific purpose/motive of the call with concrete details.
4. resolution: the exact outcome, agreement, or next step concluded between the participants.
5. resolved: boolean indicating whether the matter discussed was completed or agreed upon.
6. customer_intent: specific category (e.g., "coordination", "personal", "status_update", "scheduling", "inquiry", "support", "billing").
7. topics: array of specific topics discussed.
8. key_facts: array of 2-5 concrete bullet points stating key facts (e.g. who is where, what was moved/ordered/done, names).
9. action_items: list of commitments or next steps with owner ("speaker_1", "speaker_2", or person name), text, deadline, and evidence_segments.
10. entities: list of extracted entities (person names, locations, rooms/floors, phone numbers, prices/amounts, dates/times).
11. sentiment_score: overall tone/sentiment from -1.0 (very negative) to +1.0 (very positive).
12. risks: list of any disagreements, urgent matters, or misunderstandings identified with evidence.
13. promises: explicit promises, agreements, or commitments made between speakers.
"#
    )
}

/// Build the structured prompt for a full call analysis.
#[must_use]
pub fn build_analysis_prompt(transcript_text: &str, organization_name: &str) -> String {
    build_language_aware_analysis_prompt(transcript_text, organization_name, &Language::Hebrew)
}
