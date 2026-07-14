Feature: Inspect and actively check a tracked app
  Getter users can inspect cached app state and explicitly refresh an installed app.

  Background:
    Given an initialized getter data directory
    And a package-directory repository "official" with package "android/app/fdroid"
    And repository "official" is registered with priority 0
    And package "android/app/fdroid" is actively tracked

  Scenario: Show cached app state
    Given an installed inventory with Android app "org.fdroid.fdroid" labeled "F-Droid"
    When I run getter app show for "android/app/fdroid" with that inventory
    Then the app result names "F-Droid" and reports installed version "1.0.0"

  Scenario: Check an installed app for updates
    Given an installed inventory with Android app "org.fdroid.fdroid" labeled "F-Droid"
    When I run getter app check for "android/app/fdroid" with that inventory
    Then the app check reports an available update and issues an action

  Scenario: Check requires installed inventory
    When I run getter app check for "android/app/fdroid" without inventory
    Then the command fails with stable error "inventory.invalid"
