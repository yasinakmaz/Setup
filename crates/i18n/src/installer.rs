//! Strings of the built-in installer templates.
//!
//! Every template that uses only these messages supports all seven built-in
//! languages; see [`SUPPORTED_LANGUAGES`].

use crate::Language;

/// Languages fully translated by [`Msg`].
pub const SUPPORTED_LANGUAGES: [Language; 7] = Language::ALL;

crate::define_messages! {
    /// A user-visible installer message.
    pub enum Msg {
        SetupTitle => {
            en: "{0} Setup", tr: "{0} Kurulumu", ar: "إعداد {0}", es: "Instalación de {0}",
            fr: "Installation de {0}", de: "{0} Setup", ru: "Установка {0}",
        },
        Version => {
            en: "Version", tr: "Sürüm", ar: "الإصدار", es: "Versión",
            fr: "Version", de: "Version", ru: "Версия",
        },
        Size => {
            en: "Size", tr: "Boyut", ar: "الحجم", es: "Tamaño",
            fr: "Taille", de: "Größe", ru: "Размер",
        },
        InstallLocation => {
            en: "Install location", tr: "Kurulum konumu", ar: "موقع التثبيت",
            es: "Ubicación de instalación", fr: "Emplacement d'installation",
            de: "Installationsort", ru: "Папка установки",
        },
        Change => {
            en: "Change", tr: "Değiştir", ar: "تغيير", es: "Cambiar",
            fr: "Changer", de: "Ändern", ru: "Изменить",
        },
        Install => {
            en: "Install", tr: "Kur", ar: "تثبيت", es: "Instalar",
            fr: "Installer", de: "Installieren", ru: "Установить",
        },
        AdvancedOptions => {
            en: "Advanced options", tr: "Gelişmiş seçenekler", ar: "خيارات متقدمة",
            es: "Opciones avanzadas", fr: "Options avancées", de: "Erweiterte Optionen",
            ru: "Дополнительные параметры",
        },
        Installing => {
            en: "Installing…", tr: "Kuruluyor…", ar: "جارٍ التثبيت…", es: "Instalando…",
            fr: "Installation…", de: "Wird installiert…", ru: "Установка…",
        },
        StepPreparing => {
            en: "Preparing files", tr: "Dosyalar hazırlanıyor", ar: "تجهيز الملفات",
            es: "Preparando archivos", fr: "Préparation des fichiers",
            de: "Dateien werden vorbereitet", ru: "Подготовка файлов",
        },
        StepPrerequisites => {
            en: "Installing required components", tr: "Gerekli bileşenler kuruluyor",
            ar: "تثبيت المكونات المطلوبة", es: "Instalando componentes necesarios",
            fr: "Installation des composants requis",
            de: "Erforderliche Komponenten werden installiert",
            ru: "Установка необходимых компонентов",
        },
        StepDownloading => {
            en: "Downloading {0}", tr: "{0} indiriliyor", ar: "جارٍ تنزيل {0}",
            es: "Descargando {0}", fr: "Téléchargement de {0}",
            de: "{0} wird heruntergeladen", ru: "Загрузка {0}",
        },
        StepExtracting => {
            en: "Copying application files", tr: "Uygulama dosyaları kopyalanıyor",
            ar: "نسخ ملفات التطبيق", es: "Copiando archivos de la aplicación",
            fr: "Copie des fichiers de l'application", de: "Anwendungsdateien werden kopiert",
            ru: "Копирование файлов приложения",
        },
        StepDatabase => {
            en: "Configuring database", tr: "Veritabanı yapılandırılıyor",
            ar: "تهيئة قاعدة البيانات", es: "Configurando la base de datos",
            fr: "Configuration de la base de données", de: "Datenbank wird konfiguriert",
            ru: "Настройка базы данных",
        },
        StepTasks => {
            en: "Running configuration tasks", tr: "Yapılandırma görevleri çalıştırılıyor",
            ar: "تنفيذ مهام الإعداد", es: "Ejecutando tareas de configuración",
            fr: "Exécution des tâches de configuration",
            de: "Konfigurationsaufgaben werden ausgeführt", ru: "Выполнение задач настройки",
        },
        StepShortcuts => {
            en: "Creating shortcuts", tr: "Kısayollar oluşturuluyor", ar: "إنشاء الاختصارات",
            es: "Creando accesos directos", fr: "Création des raccourcis",
            de: "Verknüpfungen werden erstellt", ru: "Создание ярлыков",
        },
        StepRegistering => {
            en: "Registering application", tr: "Uygulama kaydediliyor", ar: "تسجيل التطبيق",
            es: "Registrando la aplicación", fr: "Enregistrement de l'application",
            de: "Anwendung wird registriert", ru: "Регистрация приложения",
        },
        StepVerifying => {
            en: "Verifying installation", tr: "Kurulum doğrulanıyor", ar: "التحقق من التثبيت",
            es: "Verificando la instalación", fr: "Vérification de l'installation",
            de: "Installation wird überprüft", ru: "Проверка установки",
        },
        StepFinishing => {
            en: "Finishing", tr: "Tamamlanıyor", ar: "إنهاء", es: "Finalizando",
            fr: "Finalisation", de: "Wird abgeschlossen", ru: "Завершение",
        },
        StepRemoving => {
            en: "Removing files", tr: "Dosyalar kaldırılıyor", ar: "إزالة الملفات",
            es: "Eliminando archivos", fr: "Suppression des fichiers",
            de: "Dateien werden entfernt", ru: "Удаление файлов",
        },
        InstallCompleted => {
            en: "Installation completed successfully.", tr: "Kurulum başarıyla tamamlandı.",
            ar: "اكتمل التثبيت بنجاح.", es: "La instalación se completó correctamente.",
            fr: "L'installation s'est terminée avec succès.",
            de: "Die Installation wurde erfolgreich abgeschlossen.",
            ru: "Установка успешно завершена.",
        },
        OpenApplication => {
            en: "Open application", tr: "Uygulamayı aç", ar: "فتح التطبيق",
            es: "Abrir aplicación", fr: "Ouvrir l'application", de: "Anwendung öffnen",
            ru: "Открыть приложение",
        },
        LaunchAutomatically => {
            en: "Launch automatically", tr: "Otomatik başlat", ar: "التشغيل تلقائيًا",
            es: "Iniciar automáticamente", fr: "Lancer automatiquement",
            de: "Automatisch starten", ru: "Запускать автоматически",
        },
        CheckForUpdates => {
            en: "Check for updates", tr: "Güncellemeleri denetle", ar: "التحقق من وجود تحديثات",
            es: "Buscar actualizaciones", fr: "Rechercher des mises à jour",
            de: "Nach Updates suchen", ru: "Проверять обновления",
        },
        Close => {
            en: "Close", tr: "Kapat", ar: "إغلاق", es: "Cerrar",
            fr: "Fermer", de: "Schließen", ru: "Закрыть",
        },
        Cancel => {
            en: "Cancel", tr: "İptal", ar: "إلغاء", es: "Cancelar",
            fr: "Annuler", de: "Abbrechen", ru: "Отмена",
        },
        Retry => {
            en: "Retry", tr: "Yeniden dene", ar: "إعادة المحاولة", es: "Reintentar",
            fr: "Réessayer", de: "Wiederholen", ru: "Повторить",
        },
        ShowDetails => {
            en: "Show details", tr: "Ayrıntıları göster", ar: "إظهار التفاصيل",
            es: "Mostrar detalles", fr: "Afficher les détails", de: "Details anzeigen",
            ru: "Показать подробности",
        },
        HideDetails => {
            en: "Hide details", tr: "Ayrıntıları gizle", ar: "إخفاء التفاصيل",
            es: "Ocultar detalles", fr: "Masquer les détails", de: "Details ausblenden",
            ru: "Скрыть подробности",
        },
        OpenLog => {
            en: "Open log", tr: "Günlüğü aç", ar: "فتح السجل", es: "Abrir registro",
            fr: "Ouvrir le journal", de: "Protokoll öffnen", ru: "Открыть журнал",
        },
        CopyError => {
            en: "Copy error", tr: "Hatayı kopyala", ar: "نسخ الخطأ", es: "Copiar error",
            fr: "Copier l'erreur", de: "Fehler kopieren", ru: "Копировать ошибку",
        },
        InstallFailed => {
            en: "Installation failed.", tr: "Kurulum başarısız oldu.", ar: "فشل التثبيت.",
            es: "La instalación falló.", fr: "L'installation a échoué.",
            de: "Die Installation ist fehlgeschlagen.", ru: "Не удалось выполнить установку.",
        },
        ChangesReverted => {
            en: "All changes were reverted.", tr: "Tüm değişiklikler geri alındı.",
            ar: "تم التراجع عن جميع التغييرات.", es: "Se revirtieron todos los cambios.",
            fr: "Toutes les modifications ont été annulées.",
            de: "Alle Änderungen wurden rückgängig gemacht.", ru: "Все изменения отменены.",
        },
        RollbackIncomplete => {
            en: "Some changes could not be reverted. See the log for details.",
            tr: "Bazı değişiklikler geri alınamadı. Ayrıntılar için günlüğe bakın.",
            ar: "تعذّر التراجع عن بعض التغييرات. راجع السجل لمزيد من التفاصيل.",
            es: "Algunos cambios no se pudieron revertir. Consulte el registro para más detalles.",
            fr: "Certaines modifications n'ont pas pu être annulées. Consultez le journal pour plus de détails.",
            de: "Einige Änderungen konnten nicht rückgängig gemacht werden. Details finden Sie im Protokoll.",
            ru: "Некоторые изменения не удалось отменить. Подробности см. в журнале.",
        },
        ErrDamaged => {
            en: "The installer is damaged. Download it again.",
            tr: "Kurulum dosyası bozuk. Lütfen yeniden indirin.",
            ar: "برنامج التثبيت تالف. يُرجى تنزيله مرة أخرى.",
            es: "El instalador está dañado. Descárguelo de nuevo.",
            fr: "Le programme d'installation est endommagé. Téléchargez-le à nouveau.",
            de: "Das Installationsprogramm ist beschädigt. Bitte laden Sie es erneut herunter.",
            ru: "Установщик повреждён. Загрузите его заново.",
        },
        ErrDiskSpace => {
            en: "Not enough disk space. {0} required, {1} available.",
            tr: "Yeterli disk alanı yok. Gereken: {0}, kullanılabilir: {1}.",
            ar: "لا توجد مساحة كافية على القرص. المطلوب {0}، والمتاح {1}.",
            es: "No hay suficiente espacio en disco. Se necesitan {0}; hay {1} disponibles.",
            fr: "Espace disque insuffisant. {0} requis, {1} disponible.",
            de: "Nicht genügend Speicherplatz. Benötigt: {0}, verfügbar: {1}.",
            ru: "Недостаточно места на диске. Требуется {0}, доступно {1}.",
        },
        ErrAccessDenied => {
            en: "Access to the install location was denied.",
            tr: "Kurulum konumuna erişim reddedildi.",
            ar: "تم رفض الوصول إلى موقع التثبيت.",
            es: "Acceso denegado a la ubicación de instalación.",
            fr: "Accès refusé à l'emplacement d'installation.",
            de: "Zugriff auf den Installationsort verweigert.",
            ru: "Отказано в доступе к папке установки.",
        },
        ErrPrerequisiteVerify => {
            en: "{0} could not be installed. The downloaded package could not be verified. Another download source can be tried.",
            tr: "{0} kurulamadı. İndirilen paket doğrulanamadı. Başka bir indirme kaynağı denenebilir.",
            ar: "تعذّر تثبيت {0}. لم يتم التحقق من الحزمة التي تم تنزيلها. يمكن تجربة مصدر تنزيل آخر.",
            es: "No se pudo instalar {0}. No se pudo verificar el paquete descargado. Se puede probar otra fuente de descarga.",
            fr: "Impossible d'installer {0}. Le paquet téléchargé n'a pas pu être vérifié. Une autre source de téléchargement peut être essayée.",
            de: "{0} konnte nicht installiert werden. Das heruntergeladene Paket konnte nicht verifiziert werden. Eine andere Downloadquelle kann versucht werden.",
            ru: "Не удалось установить {0}. Загруженный пакет не прошёл проверку. Можно попробовать другой источник загрузки.",
        },
        ErrPrerequisiteDownload => {
            en: "{0} could not be downloaded. Check your internet connection.",
            tr: "{0} indirilemedi. İnternet bağlantınızı kontrol edin.",
            ar: "تعذّر تنزيل {0}. تحقق من اتصالك بالإنترنت.",
            es: "No se pudo descargar {0}. Compruebe su conexión a Internet.",
            fr: "Impossible de télécharger {0}. Vérifiez votre connexion Internet.",
            de: "{0} konnte nicht heruntergeladen werden. Überprüfen Sie Ihre Internetverbindung.",
            ru: "Не удалось загрузить {0}. Проверьте подключение к Интернету.",
        },
        ErrPrerequisiteInstall => {
            en: "{0} could not be installed.", tr: "{0} kurulamadı.", ar: "تعذّر تثبيت {0}.",
            es: "No se pudo instalar {0}.", fr: "Impossible d'installer {0}.",
            de: "{0} konnte nicht installiert werden.", ru: "Не удалось установить {0}.",
        },
        ErrTaskFailed => {
            en: "A configuration task failed: {0}", tr: "Bir yapılandırma görevi başarısız oldu: {0}",
            ar: "فشلت مهمة إعداد: {0}", es: "Falló una tarea de configuración: {0}",
            fr: "Une tâche de configuration a échoué : {0}",
            de: "Eine Konfigurationsaufgabe ist fehlgeschlagen: {0}",
            ru: "Не удалось выполнить задачу настройки: {0}",
        },
        ErrUnexpected => {
            en: "An unexpected error occurred.", tr: "Beklenmeyen bir hata oluştu.",
            ar: "حدث خطأ غير متوقع.", es: "Se produjo un error inesperado.",
            fr: "Une erreur inattendue s'est produite.",
            de: "Ein unerwarteter Fehler ist aufgetreten.", ru: "Произошла непредвиденная ошибка.",
        },
        ErrUnsupportedSystem => {
            en: "This application does not support this system.",
            tr: "Bu uygulama bu sistemi desteklemiyor.",
            ar: "هذا التطبيق لا يدعم هذا النظام.",
            es: "Esta aplicación no es compatible con este sistema.",
            fr: "Cette application ne prend pas en charge ce système.",
            de: "Diese Anwendung unterstützt dieses System nicht.",
            ru: "Это приложение не поддерживает данную систему.",
        },
        Uninstall => {
            en: "Uninstall", tr: "Kaldır", ar: "إلغاء التثبيت", es: "Desinstalar",
            fr: "Désinstaller", de: "Deinstallieren", ru: "Удалить",
        },
        Uninstalling => {
            en: "Uninstalling…", tr: "Kaldırılıyor…", ar: "جارٍ إلغاء التثبيت…",
            es: "Desinstalando…", fr: "Désinstallation…", de: "Wird deinstalliert…",
            ru: "Удаление…",
        },
        UninstallCompleted => {
            en: "{0} was removed from your computer.", tr: "{0} bilgisayarınızdan kaldırıldı.",
            ar: "تمت إزالة {0} من جهازك.", es: "{0} se eliminó de su equipo.",
            fr: "{0} a été supprimé de votre ordinateur.",
            de: "{0} wurde von Ihrem Computer entfernt.", ru: "{0} удалено с компьютера.",
        },
        UninstallConfirm => {
            en: "Remove {0} and all of its components?",
            tr: "{0} ve tüm bileşenleri kaldırılsın mı?",
            ar: "هل تريد إزالة {0} وجميع مكوناته؟",
            es: "¿Desea eliminar {0} y todos sus componentes?",
            fr: "Supprimer {0} et tous ses composants ?",
            de: "{0} und alle zugehörigen Komponenten entfernen?",
            ru: "Удалить {0} и все его компоненты?",
        },
        AlreadyInstalled => {
            en: "{0} {1} is already installed.", tr: "{0} {1} zaten kurulu.",
            ar: "{0} {1} مثبت بالفعل.", es: "{0} {1} ya está instalado.",
            fr: "{0} {1} est déjà installé.", de: "{0} {1} ist bereits installiert.",
            ru: "{0} {1} уже установлено.",
        },
        UpgradeNotice => {
            en: "Version {0} will be updated to {1}.",
            tr: "{0} sürümü {1} sürümüne güncellenecek.",
            ar: "سيتم تحديث الإصدار {0} إلى {1}.",
            es: "La versión {0} se actualizará a {1}.",
            fr: "La version {0} sera mise à jour vers {1}.",
            de: "Version {0} wird auf {1} aktualisiert.",
            ru: "Версия {0} будет обновлена до {1}.",
        },
        LanguageLabel => {
            en: "Language", tr: "Dil", ar: "اللغة", es: "Idioma",
            fr: "Langue", de: "Sprache", ru: "Язык",
        },
        ChooseLocation => {
            en: "Choose install location", tr: "Kurulum konumunu seçin",
            ar: "اختر موقع التثبيت", es: "Elegir ubicación de instalación",
            fr: "Choisir l'emplacement d'installation", de: "Installationsort auswählen",
            ru: "Выберите папку установки",
        },
        DesktopShortcut => {
            en: "Create desktop shortcut", tr: "Masaüstü kısayolu oluştur",
            ar: "إنشاء اختصار على سطح المكتب", es: "Crear acceso directo en el escritorio",
            fr: "Créer un raccourci sur le bureau", de: "Desktopverknüpfung erstellen",
            ru: "Создать ярлык на рабочем столе",
        },
        RequiresAdmin => {
            en: "Administrator permission is required.", tr: "Yönetici izni gerekiyor.",
            ar: "يلزم الحصول على إذن المسؤول.", es: "Se requieren permisos de administrador.",
            fr: "Une autorisation d'administrateur est requise.",
            de: "Administratorrechte sind erforderlich.", ru: "Требуются права администратора.",
        },
        Back => {
            en: "Back", tr: "Geri", ar: "رجوع", es: "Atrás",
            fr: "Retour", de: "Zurück", ru: "Назад",
        },
        FreeSpace => {
            en: "Available: {0}", tr: "Kullanılabilir: {0}", ar: "المتاح: {0}",
            es: "Disponible: {0}", fr: "Disponible : {0}", de: "Verfügbar: {0}",
            ru: "Доступно: {0}",
        },
        Repair => {
            en: "Repair", tr: "Onar", ar: "إصلاح", es: "Reparar",
            fr: "Réparer", de: "Reparieren", ru: "Восстановить",
        },
        Modify => {
            en: "Modify", tr: "Düzenle", ar: "تعديل", es: "Modificar",
            fr: "Modifier", de: "Ändern", ru: "Изменить",
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
    fn every_language_is_translated_and_keeps_placeholders() {
        fn placeholders(s: &str) -> [bool; 4] {
            ["{0}", "{1}", "{2}", "{3}"].map(|p| s.contains(p))
        }
        for msg in Msg::ALL {
            let en = msg.text(Language::En);
            for lang in Language::ALL {
                let text = msg.text(lang);
                assert!(!text.trim().is_empty(), "{msg:?} empty in {lang}");
                assert_eq!(
                    placeholders(text),
                    placeholders(en),
                    "{msg:?}: placeholder mismatch in {lang}"
                );
            }
        }
        // A spot check that the tables are really per language.
        assert_eq!(Msg::Install.text(Language::Tr), "Kur");
        assert_eq!(Msg::Install.text(Language::Ar), "تثبيت");
    }
}
