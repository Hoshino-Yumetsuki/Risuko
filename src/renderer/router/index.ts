import { createRouter, createWebHashHistory } from "vue-router";
import Main from "@/components/Main.vue";
import TaskIndex from "@/components/Task/Index.vue";

export default createRouter({
	history: createWebHashHistory(),
	routes: [
		{
			path: "/",
			name: "main",
			component: Main,
			children: [
				{
					path: "/task",
					alias: "/",
					component: TaskIndex,
					props: {
						status: "all",
					},
				},
				{
					path: "/task/:status",
					name: "task",
					component: TaskIndex,
					props: true,
				},
				{
					path: "/rss",
					name: "rss",
					component: () => import("@/components/Rss/Index.vue"),
				},
				{
					path: "/share",
					name: "share",
					component: () => import("@/components/Share/Index.vue"),
				},
				{
					path: "/health",
					name: "health",
					component: () => import("@/components/Health/Index.vue"),
				},
				{
					path: "/preference",
					name: "preference",
					component: () => import("@/components/Preference/Index.vue"),
					redirect: "/preference/basic",
					props: true,
					children: [
						{
							path: "basic",
							alias: "",
							components: {
								subnav: () =>
									import("@/components/Subnav/PreferenceSubnav.vue"),
								form: () => import("@/components/Preference/Basic.vue"),
							},
							props: {
								subnav: { current: "basic" },
							},
						},
						{
							path: "advanced",
							components: {
								subnav: () =>
									import("@/components/Subnav/PreferenceSubnav.vue"),
								form: () => import("@/components/Preference/Advanced.vue"),
							},
							props: {
								subnav: { current: "advanced" },
							},
						},
						{
							path: "cloud-sinks",
							components: {
								subnav: () =>
									import("@/components/Subnav/PreferenceSubnav.vue"),
								form: () => import("@/components/Preference/CloudSinks.vue"),
							},
							props: {
								subnav: { current: "cloud-sinks" },
							},
						},
						{
							path: "sync",
							components: {
								subnav: () =>
									import("@/components/Subnav/PreferenceSubnav.vue"),
								form: () => import("@/components/Preference/Sync.vue"),
							},
							props: {
								subnav: { current: "sync" },
							},
						},
					],
				},
			],
		},
		{
			path: "/:pathMatch(.*)*",
			redirect: "/",
		},
	],
});
