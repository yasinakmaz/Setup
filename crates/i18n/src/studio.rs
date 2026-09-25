//! Strings of the Installer Studio chrome (menus, panels, statuses).
//!
//! Separate from [`crate::installer`]: the Studio is a desktop application
//! that always ships all seven languages, while a generated installer
//! compiles in only the ones its project enables.

use crate::Language;

/// Languages fully translated by [`Msg`].
pub const SUPPORTED_LANGUAGES: [Language; 7] = Language::ALL;

crate::define_messages! {
    /// A user-visible Installer Studio message.
    pub enum Msg {
        New => {
            en: "New", tr: "Yeni", ar: "جديد", es: "Nuevo",
            fr: "Nouveau", de: "Neu", ru: "Создать",
        },
        Open => {
            en: "Open", tr: "Aç", ar: "فتح", es: "Abrir",
            fr: "Ouvrir", de: "Öffnen", ru: "Открыть",
        },
        Save => {
            en: "Save", tr: "Kaydet", ar: "حفظ", es: "Guardar",
            fr: "Enregistrer", de: "Speichern", ru: "Сохранить",
        },
        Build => {
            en: "Build", tr: "Derle", ar: "إنشاء", es: "Compilar",
            fr: "Générer", de: "Erstellen", ru: "Сборка",
        },
        Analyze => {
            en: "Analyze", tr: "Analiz Et", ar: "تحليل", es: "Analizar",
            fr: "Analyser", de: "Analysieren", ru: "Анализ",
        },
        RunDoctor => {
            en: "Run Doctor", tr: "Doktoru Çalıştır", ar: "تشغيل الفحص",
            es: "Ejecutar diagnóstico", fr: "Lancer le diagnostic",
            de: "Diagnose ausführen", ru: "Запустить диагностику",
        },
        CommandPalette => {
            en: "Command Palette", tr: "Komut Paleti", ar: "لوحة الأوامر",
            es: "Paleta de comandos", fr: "Palette de commandes",
            de: "Befehlspalette", ru: "Палитра команд",
        },
        Explorer => {
            en: "Explorer", tr: "Gezgin", ar: "المستكشف", es: "Explorador",
            fr: "Explorateur", de: "Explorer", ru: "Обозреватель",
        },
        Properties => {
            en: "Properties", tr: "Özellikler", ar: "الخصائص", es: "Propiedades",
            fr: "Propriétés", de: "Eigenschaften", ru: "Свойства",
        },
        Output => {
            en: "Output", tr: "Çıktı", ar: "المخرجات", es: "Salida",
            fr: "Sortie", de: "Ausgabe", ru: "Вывод",
        },
        Problems => {
            en: "Problems", tr: "Sorunlar", ar: "المشكلات", es: "Problemas",
            fr: "Problèmes", de: "Probleme", ru: "Проблемы",
        },
        BuildTab => {
            en: "Build", tr: "Derleme", ar: "الإنشاء", es: "Compilación",
            fr: "Génération", de: "Erstellung", ru: "Сборка",
        },
        Doctor => {
            en: "Doctor", tr: "Doktor", ar: "الفحص", es: "Diagnóstico",
            fr: "Diagnostic", de: "Diagnose", ru: "Диагностика",
        },
        Simple => {
            en: "Simple", tr: "Basit", ar: "بسيط", es: "Simple",
            fr: "Simple", de: "Einfach", ru: "Простой",
        },
        Advanced => {
            en: "Advanced", tr: "Gelişmiş", ar: "متقدم", es: "Avanzado",
            fr: "Avancé", de: "Erweitert", ru: "Расширенный",
        },
        NoProjectOpen => {
            en: "No project open. Use New or Open to get started.",
            tr: "Açık proje yok. Başlamak için Yeni veya Aç kullanın.",
            ar: "لا يوجد مشروع مفتوح. استخدم جديد أو فتح للبدء.",
            es: "No hay ningún proyecto abierto. Use Nuevo o Abrir para empezar.",
            fr: "Aucun projet ouvert. Utilisez Nouveau ou Ouvrir pour commencer.",
            de: "Kein Projekt geöffnet. Verwenden Sie Neu oder Öffnen, um zu beginnen.",
            ru: "Проект не открыт. Нажмите «Создать» или «Открыть», чтобы начать.",
        },
        Ready => {
            en: "Ready", tr: "Hazır", ar: "جاهز", es: "Listo",
            fr: "Prêt", de: "Bereit", ru: "Готово",
        },
        BuildSucceeded => {
            en: "Build succeeded.", tr: "Derleme başarılı.", ar: "نجح الإنشاء.",
            es: "Compilación correcta.", fr: "Génération réussie.",
            de: "Build erfolgreich.", ru: "Сборка успешно завершена.",
        },
        BuildFailed => {
            en: "Build failed.", tr: "Derleme başarısız oldu.", ar: "فشل الإنشاء.",
            es: "Error en la compilación.", fr: "Échec de la génération.",
            de: "Build fehlgeschlagen.", ru: "Сборка не удалась.",
        },
        DoctorNoIssues => {
            en: "No issues found.", tr: "Sorun bulunamadı.", ar: "لم يتم العثور على مشكلات.",
            es: "No se encontraron problemas.", fr: "Aucun problème détecté.",
            de: "Keine Probleme gefunden.", ru: "Проблем не обнаружено.",
        },
        ApplyFix => {
            en: "Apply fix", tr: "Düzeltmeyi uygula", ar: "تطبيق الإصلاح",
            es: "Aplicar solución", fr: "Appliquer la correction",
            de: "Korrektur anwenden", ru: "Применить исправление",
        },
        ApplyAllSafeFixes => {
            en: "Apply all safe fixes", tr: "Tüm güvenli düzeltmeleri uygula",
            ar: "تطبيق كل الإصلاحات الآمنة", es: "Aplicar todas las soluciones seguras",
            fr: "Appliquer toutes les corrections sûres",
            de: "Alle sicheren Korrekturen anwenden",
            ru: "Применить все безопасные исправления",
        },
        ProductSection => {
            en: "Product", tr: "Ürün", ar: "المنتج", es: "Producto",
            fr: "Produit", de: "Produkt", ru: "Продукт",
        },
        InstallSection => {
            en: "Install", tr: "Kurulum", ar: "التثبيت", es: "Instalación",
            fr: "Installation", de: "Installation", ru: "Установка",
        },
        UiSection => {
            en: "User Interface", tr: "Kullanıcı Arayüzü", ar: "واجهة المستخدم",
            es: "Interfaz de usuario", fr: "Interface utilisateur",
            de: "Benutzeroberfläche", ru: "Интерфейс",
        },
        PolicySection => {
            en: "Policy", tr: "İlke", ar: "السياسة", es: "Política",
            fr: "Politique", de: "Richtlinie", ru: "Политика",
        },
        TargetsSection => {
            en: "Targets", tr: "Hedefler", ar: "الأهداف", es: "Destinos",
            fr: "Cibles", de: "Zielsysteme", ru: "Платформы",
        },
        CompressionSection => {
            en: "Compression", tr: "Sıkıştırma", ar: "الضغط", es: "Compresión",
            fr: "Compression", de: "Komprimierung", ru: "Сжатие",
        },
        SigningSection => {
            en: "Signing", tr: "İmzalama", ar: "التوقيع", es: "Firma",
            fr: "Signature", de: "Signierung", ru: "Подпись",
        },
        UpdateSection => {
            en: "Updates", tr: "Güncellemeler", ar: "التحديثات", es: "Actualizaciones",
            fr: "Mises à jour", de: "Updates", ru: "Обновления",
        },
        Name => {
            en: "Name", tr: "Ad", ar: "الاسم", es: "Nombre",
            fr: "Nom", de: "Name", ru: "Название",
        },
        Identifier => {
            en: "Identifier", tr: "Tanımlayıcı", ar: "المعرّف", es: "Identificador",
            fr: "Identifiant", de: "Kennung", ru: "Идентификатор",
        },
        Publisher => {
            en: "Publisher", tr: "Yayımcı", ar: "الناشر", es: "Editor",
            fr: "Éditeur", de: "Herausgeber", ru: "Издатель",
        },
        Description => {
            en: "Description", tr: "Açıklama", ar: "الوصف", es: "Descripción",
            fr: "Description", de: "Beschreibung", ru: "Описание",
        },
        AllowChangeLocation => {
            en: "Allow changing the install location",
            tr: "Kurulum konumunun değiştirilmesine izin ver",
            ar: "السماح بتغيير موقع التثبيت",
            es: "Permitir cambiar la ubicación de instalación",
            fr: "Autoriser à changer l'emplacement d'installation",
            de: "Ändern des Installationsorts erlauben",
            ru: "Разрешить изменение папки установки",
        },
        Languages => {
            en: "Languages", tr: "Diller", ar: "اللغات", es: "Idiomas",
            fr: "Langues", de: "Sprachen", ru: "Языки",
        },
        FallbackLanguage => {
            en: "Fallback language", tr: "Yedek dil", ar: "اللغة الاحتياطية",
            es: "Idioma de reserva", fr: "Langue de repli",
            de: "Ausweichsprache", ru: "Резервный язык",
        },
        Rollback => {
            en: "Roll back on failure", tr: "Hata durumunda geri al",
            ar: "التراجع عند الفشل", es: "Revertir en caso de error",
            fr: "Annuler en cas d'échec", de: "Bei Fehler zurücksetzen",
            ru: "Откат при сбое",
        },
        SilentInstall => {
            en: "Allow silent install", tr: "Sessiz kuruluma izin ver",
            ar: "السماح بالتثبيت الصامت", es: "Permitir instalación silenciosa",
            fr: "Autoriser l'installation silencieuse",
            de: "Stille Installation erlauben", ru: "Разрешить тихую установку",
        },
        SilentUninstall => {
            en: "Allow silent uninstall", tr: "Sessiz kaldırmaya izin ver",
            ar: "السماح بإلغاء التثبيت الصامت", es: "Permitir desinstalación silenciosa",
            fr: "Autoriser la désinstallation silencieuse",
            de: "Stille Deinstallation erlauben", ru: "Разрешить тихое удаление",
        },
        Telemetry => {
            en: "Telemetry", tr: "Telemetri", ar: "القياس عن بُعد", es: "Telemetría",
            fr: "Télémétrie", de: "Telemetrie", ru: "Телеметрия",
        },
        CompressionProfile => {
            en: "Profile", tr: "Profil", ar: "الملف الشخصي", es: "Perfil",
            fr: "Profil", de: "Profil", ru: "Профиль",
        },
        RequireSigned => {
            en: "Require a signed installer", tr: "İmzalı kurulum gerektir",
            ar: "طلب مثبت موقّع", es: "Requerir instalador firmado",
            fr: "Exiger un installateur signé", de: "Signierten Installer erfordern",
            ru: "Требовать подписанный установщик",
        },
        CheckForUpdates => {
            en: "Check for updates by default", tr: "Varsayılan olarak güncellemeleri denetle",
            ar: "التحقق من التحديثات افتراضيًا", es: "Buscar actualizaciones de forma predeterminada",
            fr: "Rechercher les mises à jour par défaut",
            de: "Standardmäßig nach Updates suchen", ru: "Проверять обновления по умолчанию",
        },
        UnsavedChanges => {
            en: "Unsaved changes", tr: "Kaydedilmemiş değişiklikler", ar: "تغييرات غير محفوظة",
            es: "Cambios sin guardar", fr: "Modifications non enregistrées",
            de: "Nicht gespeicherte Änderungen", ru: "Несохранённые изменения",
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn english_is_always_available() {
        for msg in Msg::ALL {
            assert!(!msg.text(Language::En).is_empty());
        }
    }

    #[cfg(feature = "all-languages")]
    #[test]
    fn every_language_is_translated() {
        for msg in Msg::ALL {
            for lang in Language::ALL {
                assert!(!msg.text(lang).trim().is_empty(), "{msg:?} empty in {lang}");
            }
        }
        assert_eq!(Msg::Save.text(Language::Tr), "Kaydet");
        assert_eq!(Msg::Save.text(Language::Ar), "حفظ");
    }
}
